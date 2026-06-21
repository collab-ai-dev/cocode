use std::collections::HashMap;
use std::sync::Arc;

use coco_hooks::HookExecutionEvent;
use coco_hooks::HookRegistry;
use coco_hooks::orchestration::OrchestrationContext;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolInputInvalidReason;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_permissions::AutoModeRules;
use coco_tool_runtime::CanUseToolDecision;
use coco_tool_runtime::DecisionReason;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PendingToolCall;
use coco_tool_runtime::ToolPermissionBridgeRef;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidatedInput;
use coco_types::CoreEvent;
use coco_types::ModelRole;
use coco_types::PermissionDecision;
use coco_types::PermissionDenialInfo;
use coco_types::ToolId;
use coco_types::ToolName;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_with_error_mode;
use crate::hook_controller::HookController;
use crate::hook_controller::PreToolUseOutcome;
use crate::permission_controller::PermissionController;
use crate::permission_controller::PermissionOutcome;
use crate::session_state::SessionStateTracker;
use crate::tool_runner::prepare_committed_tool_call;

/// Per-call data carried from preparation → run_one → on_outcome.
///
/// Keyed by `tool_use_id`, this lets the runner retrieve the
/// post-hook `tool_name` + effective input that the preparer
/// resolved, without re-deriving them in `run_one`. `is_mcp` is
/// asked directly from the tool via `Tool::is_mcp()` at outcome-
/// build time (single source of truth), so it does not travel
/// through this struct.
#[derive(Debug, Clone)]
pub(crate) struct ToolResultContext {
    pub tool_name: String,
    pub effective_input: ValidatedInput,
    pub approval_feedback: Option<String>,
    pub permission_resolution_detail: Option<coco_types::PermissionResolutionDetail>,
}

pub(crate) struct PendingToolPreparation<'a> {
    pub event_tx: &'a Option<mpsc::Sender<CoreEvent>>,
    pub history: &'a mut MessageHistory,
    pub ctx: &'a ToolUseContext,
    pub tool_calls: &'a [ToolCallPart],
    pub tools: &'a ToolRegistry,
    pub hooks: Option<&'a Arc<HookRegistry>>,
    pub orchestration_ctx: OrchestrationContext,
    pub hook_tx_opt: Option<&'a mpsc::Sender<HookExecutionEvent>>,
    pub permission_denials: &'a mut Vec<PermissionDenialInfo>,
    pub state_tracker: &'a SessionStateTracker,
    pub permission_bridge: Option<&'a ToolPermissionBridgeRef>,
    pub session_id: &'a str,
    pub cancel: &'a CancellationToken,
    pub auto_mode_state: Option<&'a Arc<coco_permissions::AutoModeState>>,
    pub denial_tracker: Option<&'a Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
    pub model_runtimes: &'a Arc<ModelRuntimeRegistry>,
    pub auto_mode_rules: &'a AutoModeRules,
    pub completion_event_mode: ToolCompletionEventMode,
    pub deferred_tool_completions: Option<&'a mut crate::helpers::DeferredToolCompletionBuffer>,
}

pub(crate) async fn prepare_pending_tool_calls(
    mut args: PendingToolPreparation<'_>,
) -> (
    Vec<PendingToolCall>,
    HashMap<String, ToolResultContext>,
    bool,
) {
    let mut pending = Vec::new();
    let mut tool_result_contexts = HashMap::new();
    let mut permission_aborted = false;

    // Ownership gymnastics: `prepare_one_pending_tool_call` borrows
    // the args struct mutably for per-call state (history +
    // permission_denials). We split the tool_calls slice out first
    // so the inner loop can re-borrow args freely.
    let tool_calls = args.tool_calls;
    for tc in tool_calls {
        if let Some((pending_call, ctx)) =
            prepare_one_pending_tool_call(&mut args, tc, &mut permission_aborted).await
        {
            tool_result_contexts.insert(tc.tool_call_id.clone(), ctx);
            pending.push(pending_call);
        }
    }

    (pending, tool_result_contexts, permission_aborted)
}

/// Run the full per-tool preparation pipeline (validate → pre-hook →
/// input rewrite → re-validate → permission → bridge) against one
/// committed assistant tool_use.
///
/// Returns `Some((PendingToolCall, ToolResultContext))` when the
/// call made it through all gates and is ready for execution;
/// `None` when preparation failed — in which case an error
/// tool_result has already been pushed to history via
/// `complete_tool_call_with_error` and the caller should simply
/// skip the call.
///
/// This is the reusable per-call body used by both:
/// - `prepare_pending_tool_calls` (batch-at-end non-streaming path)
/// - Phase 9 streaming path (to be wired — call once per
///   `ToolCallEnd` event so safe tools can start mid-stream)
///
/// Extracting this makes the streaming integration trivial: the
/// engine's stream consumer calls this function as each tool_use
/// block arrives, feeding the result into `StreamingHandle`. The
/// preparation semantics (pre-hook order, permission path, audit
/// records) stay byte-identical between streaming and
/// non-streaming, which is the plan's I2 invariant.
pub(crate) async fn prepare_one_pending_tool_call(
    args: &mut PendingToolPreparation<'_>,
    tc: &ToolCallPart,
    permission_aborted: &mut bool,
) -> Option<(PendingToolCall, ToolResultContext)> {
    let prepared = prepare_committed_tool_call(
        args.event_tx,
        args.history,
        args.tools,
        args.ctx,
        tc,
        args.completion_event_mode,
        args.deferred_tool_completions.as_deref_mut(),
    )
    .await?;

    let tool_id = prepared.tool_id;
    let tool = prepared.tool;
    let validated_input = prepared.input;

    // schema validation already ran inside
    // `prepare_committed_tool_call` (tool_runner.rs:82-123) — it
    // returns `None` on `invalid=true` after emitting the synthetic
    // `<tool_use_error>...>` tool_result, so by reaching this point
    // we know the call is structurally valid. No duplicate validation
    // here; the remaining short-circuit handles the rare case where
    // wire parsing set `invalid=true` AFTER `prepare_committed_tool_call`
    // returned (cannot happen in current code paths, but keeps the
    // invariant local).
    if tc.invalid {
        let message = match &tc.invalid_reason {
            Some(ToolInputInvalidReason::SchemaViolation { message }) => {
                format!("<tool_use_error>InputValidationError: {message}</tool_use_error>")
            }
            Some(ToolInputInvalidReason::NoSuchTool { tool_name }) => {
                format!("<tool_use_error>No such tool available: {tool_name}</tool_use_error>")
            }
            Some(ToolInputInvalidReason::JsonParseFailed { error, .. }) => {
                format!(
                    "<tool_use_error>The tool call arguments could not be parsed as JSON: {error}. \
                     Please retry with valid JSON.</tool_use_error>"
                )
            }
            None => {
                // Legacy path: invalid=true with no structured reason.
                "<tool_use_error>The tool call arguments could not be parsed as JSON, \
                 even after repair. Please retry with valid JSON arguments.</tool_use_error>"
                    .to_string()
            }
        };
        crate::helpers::complete_tool_call_with_error_mode(
            args.event_tx,
            args.history,
            &tc.tool_call_id,
            &tc.tool_name,
            &tool_id,
            &message,
            coco_tool_runtime::ToolCallErrorKind::SchemaFailed,
            args.completion_event_mode,
            args.deferred_tool_completions.as_deref_mut(),
        )
        .await;
        return None;
    }
    let hook_controller =
        HookController::new(args.hooks, args.orchestration_ctx.clone(), args.hook_tx_opt);
    let pre_tool_outcome = hook_controller
        .run_pre_tool_use(
            args.event_tx,
            args.history,
            tc,
            &tool_id,
            args.completion_event_mode,
            args.deferred_tool_completions.as_deref_mut(),
        )
        .await;

    let (effective_input, hook_permission_behavior, hook_permission_reason) =
        resolve_effective_input_from_pre_hook(
            args.event_tx,
            args.history,
            args.ctx,
            tc,
            &tool_id,
            &tool,
            validated_input,
            pre_tool_outcome,
            args.completion_event_mode,
            args.deferred_tool_completions.as_deref_mut(),
        )
        .await?;

    let decision = resolve_permission_decision(
        tc,
        &tool,
        effective_input.as_value(),
        args.ctx,
        args.history.as_slice(),
        (hook_permission_behavior, hook_permission_reason),
        args.auto_mode_state,
        args.denial_tracker,
        args.model_runtimes,
        args.auto_mode_rules,
        args.tools,
    )
    .await;

    // When an auto-mode classifier denial lands, fire `PermissionDenied`
    // hooks. If any hook returns `retry: true`, the model is hinted that
    // it may retry. We extend the deny message in-place so the existing
    // controller path stays unchanged.
    let decision = maybe_fire_permission_denied_hook(
        &hook_controller,
        tc,
        effective_input.as_value(),
        decision,
    )
    .await;

    let permission_outcome = PermissionController::new(
        args.event_tx,
        args.history,
        args.permission_denials,
        args.state_tracker,
        args.permission_bridge,
        args.session_id,
        args.cancel,
        args.hooks,
        Some(&args.orchestration_ctx),
        args.ctx
            .cwd_override
            .as_deref()
            .or(args.ctx.original_cwd.as_deref())
            .and_then(|p| p.to_str())
            .map(str::to_owned),
        args.completion_event_mode,
        args.ctx.avoid_permission_prompts,
        args.deferred_tool_completions.as_deref_mut(),
    )
    .resolve(decision, tc, effective_input.as_value(), &tool_id)
    .await;
    if matches!(permission_outcome, PermissionOutcome::Aborted) {
        *permission_aborted = true;
        return None;
    }

    let (effective_input, approval_feedback, permission_resolution_detail) =
        resolve_effective_input_from_permission(
            args.event_tx,
            args.history,
            args.ctx,
            tc,
            &tool_id,
            &tool,
            permission_outcome,
            effective_input,
            args.completion_event_mode,
            args.deferred_tool_completions.as_deref_mut(),
        )
        .await?;

    Some((
        PendingToolCall {
            tool_use_id: tc.tool_call_id.clone(),
            tool: tool.clone(),
            input: effective_input.clone(),
            is_concurrency_safe: dynamic_concurrency_safe(
                tool.as_ref(),
                &tool_id,
                effective_input.as_value(),
                args.ctx,
            )
            .await,
        },
        ToolResultContext {
            tool_name: tc.tool_name.clone(),
            effective_input,
            approval_feedback,
            permission_resolution_detail,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
async fn resolve_effective_input_from_pre_hook(
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    history: &mut MessageHistory,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    tool_id: &ToolId,
    tool: &Arc<dyn DynTool>,
    validated_input: ValidatedInput,
    pre_tool_outcome: PreToolUseOutcome,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<(
    ValidatedInput,
    Option<coco_types::PermissionBehavior>,
    Option<String>,
)> {
    match pre_tool_outcome {
        PreToolUseOutcome::Blocked => None,
        PreToolUseOutcome::Continue {
            updated_input,
            permission_behavior,
            reason,
        } => {
            if let Some(updated_input) = updated_input {
                return validate_effective_input_or_complete_error(
                    event_tx,
                    history,
                    ctx,
                    tool_call,
                    tool_id,
                    tool,
                    updated_input,
                    completion_event_mode,
                    deferred_tool_completions,
                )
                .await
                .map(|input| (input, permission_behavior, reason));
            }
            // No hook rewrite: the prepared call's coerced, schema-validated
            // input flows through unchanged. (Previously this fell back to
            // the RAW `tool_call.input`, which silently discarded freeform
            // coercion — apply_patch's `{patch: …}` wrap — and broke both
            // path-based permission carve-outs and execute-time
            // deserialization.)
            Some((validated_input, permission_behavior, reason))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_permission_decision<M: std::borrow::Borrow<Message>>(
    tool_call: &ToolCallPart,
    tool: &Arc<dyn DynTool>,
    effective_input: &Value,
    ctx: &ToolUseContext,
    history_messages: &[M],
    hook_permission: (Option<coco_types::PermissionBehavior>, Option<String>),
    auto_mode_state: Option<&Arc<coco_permissions::AutoModeState>>,
    denial_tracker: Option<&Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    auto_mode_rules: &AutoModeRules,
    tools: &ToolRegistry,
) -> PermissionDecision {
    let (hook_permission_behavior, hook_permission_reason) = hook_permission;
    let mut hook_permission_behavior = hook_permission_behavior;

    // Subagent/fork isolation: prefer `ctx.local_denial_tracking` over the
    // engine-level session tracker. Without this, a fork's denials would bump
    // the parent's consecutive-denial circuit breaker.
    let chosen_tracker: Option<Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>> = ctx
        .local_denial_tracking
        .clone()
        .or_else(|| denial_tracker.cloned());

    if let Some(gate) =
        resolve_can_use_tool_decision(tool_call, effective_input, ctx, hook_permission_behavior)
            .await
    {
        match gate {
            CanUseToolResolution::Decision(decision) => {
                reset_consecutive_on_allow(&decision, auto_mode_state, chosen_tracker.as_ref())
                    .await;
                return decision;
            }
            CanUseToolResolution::Ask => {
                if matches!(
                    hook_permission_behavior,
                    Some(coco_types::PermissionBehavior::Allow)
                ) && ctx.require_can_use_tool
                {
                    hook_permission_behavior = None;
                }
            }
        }
    }

    let mut decision = match hook_permission_behavior {
        Some(coco_types::PermissionBehavior::Allow) => PermissionDecision::Allow {
            updated_input: None,
            feedback: hook_permission_reason,
        },
        Some(coco_types::PermissionBehavior::Ask) => PermissionDecision::Ask {
            message: hook_permission_reason
                .unwrap_or_else(|| "PreToolUse hook requested approval".into()),
            suggestions: Vec::new(),
            choices: None,
            detail: None,
        },
        Some(coco_types::PermissionBehavior::Deny) => PermissionDecision::Deny {
            message: hook_permission_reason
                .clone()
                .unwrap_or_else(|| "PreToolUse hook denied tool execution".into()),
            reason: coco_types::PermissionDecisionReason::Hook {
                hook_name: "PreToolUse".into(),
                reason: hook_permission_reason,
            },
        },
        None => evaluate_with_rules(tool, effective_input, ctx).await,
    };

    // Mirror TS `permissions.ts:549`: a tool that *requires* user interaction
    // (AskUserQuestion) returns `Ask` because the prompt IS the tool's function,
    // not a security gate. Auto mode exists to skip security confirmations, so it
    // must not run the classifier / safe-tool allowlist over such an `Ask` —
    // doing so rewrites it to `Allow`, the permission bridge never fires, and the
    // interactive overlay is silently dropped (the model just sees the questions
    // echoed back). Skip the overlay here so the `Ask` reaches the bridge.
    // Auto-mode classifier gate — TS parity (`permissions.ts:520`): the
    // decision to run the classifier reads the PER-CALL permission context
    // mode, never a shared session-global flag. Primary path is `mode == Auto`;
    // `mode == Plan` bridges to the classifier only when the narrowly-scoped
    // auto flag is set (mirrors TS `mode === 'plan' && isAutoModeActive()`).
    // Because each subagent carries its own `permission_context.mode`, a
    // concurrent engine build can no longer race the classifier off.
    let auto_classify = should_auto_classify(ctx.permission_context.mode, auto_mode_state);
    if matches!(decision, PermissionDecision::Ask { .. })
        && !tool.requires_user_interaction()
        && auto_classify
        && let Some(tracker) = chosen_tracker.as_ref()
    {
        let tool_id = tool.id();
        let shell_cwd = shell_analysis_cwd(&tool_id, ctx).await;
        let is_read_only = dynamic_read_only(
            tool.as_ref(),
            &tool_id,
            effective_input,
            shell_cwd.as_deref(),
        );
        // Context for path-safety immunity + safe-in-cwd fast path + headless
        // fail-closed. cwd: worktree override first, else the bootstrap cwd.
        let cwd = ctx
            .cwd_override
            .as_deref()
            .or(ctx.original_cwd.as_deref())
            .and_then(|p| p.to_str())
            .map(str::to_owned);
        let additional_dirs: Vec<String> = ctx
            .permission_context
            .additional_dirs
            .keys()
            .cloned()
            .collect();
        let auto_ctx = coco_permissions::AutoModeContext {
            cwd: cwd.as_deref(),
            additional_dirs: &additional_dirs,
            avoid_permission_prompts: ctx.avoid_permission_prompts,
        };
        // The classifier IS being consulted (per-call mode is Auto / Plan-bridge
        // + tracker present). agent_id rides the enclosing span, so a subagent's
        // calls are attributable. The matching "result" line says what happened.
        tracing::debug!(
            tool = %tool_call.tool_name,
            mode = ?ctx.permission_context.mode,
            is_read_only,
            "auto-mode classifier override: entering classify",
        );
        let mut tracker_guard = tracker.lock().await;
        let classifier_decision = try_classify_in_auto_mode(
            &tool_call.tool_name,
            effective_input,
            is_read_only,
            auto_classify,
            &mut tracker_guard,
            history_messages,
            model_runtimes,
            auto_mode_rules,
            auto_ctx,
            tools,
        )
        .await;
        drop(tracker_guard);
        // `none` ⇒ classifier declined to decide and the interactive prompt
        // stands; otherwise the classifier produced this allow/deny/ask verdict.
        tracing::debug!(
            tool = %tool_call.tool_name,
            classifier_outcome = classifier_decision
                .as_ref()
                .map_or("none(prompt_stands)", decision_label),
            "auto-mode classifier override: result",
        );
        if let Some(d) = classifier_decision {
            decision = d;
        }
    } else if matches!(decision, PermissionDecision::Ask { .. })
        && !tool.requires_user_interaction()
    {
        // An Ask that auto-mode could have classified was left as a prompt.
        // Log the per-call mode + wiring so a stray Auto-mode prompt is
        // diagnosable: a non-Auto/Plan `mode` is correct; otherwise a missing
        // tracker or unwired engine is the cause. Default module target so the
        // line is captured under the standard `coco=debug` filter.
        tracing::debug!(
            tool = %tool_call.tool_name,
            mode = ?ctx.permission_context.mode,
            auto_classify,
            has_auto_mode_state = auto_mode_state.is_some(),
            has_tracker = chosen_tracker.is_some(),
            "auto-mode classifier not run on Ask — surfacing interactive prompt",
        );
    }

    // Reset consecutive-denial counter on ANY auto-mode allow —
    // rule-based, hook, allowlist, acceptEdits fast-path, or classifier.
    // The classifier path resets internally; this covers the
    // rule/hook/allowlist Allow branches that bypass it.
    reset_consecutive_on_allow(&decision, auto_mode_state, chosen_tracker.as_ref()).await;

    decision
}

/// Reset the consecutive-denial counter when an auto-mode permission resolves
/// to `Allow`, regardless of which branch produced it. No-op outside auto mode,
/// when no tracker is wired, or when the counter is already zero.
async fn reset_consecutive_on_allow(
    decision: &PermissionDecision,
    auto_mode_state: Option<&Arc<coco_permissions::AutoModeState>>,
    chosen_tracker: Option<&Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>>,
) {
    if matches!(decision, PermissionDecision::Allow { .. })
        && let (Some(state), Some(tracker)) = (auto_mode_state, chosen_tracker)
        && state.is_active()
    {
        tracker.lock().await.reset_consecutive();
    }
}

enum CanUseToolResolution {
    Decision(PermissionDecision),
    Ask,
}

async fn resolve_can_use_tool_decision(
    tool_call: &ToolCallPart,
    effective_input: &Value,
    ctx: &ToolUseContext,
    hook_permission_behavior: Option<coco_types::PermissionBehavior>,
) -> Option<CanUseToolResolution> {
    let should_run = match hook_permission_behavior {
        Some(coco_types::PermissionBehavior::Deny) => false,
        Some(coco_types::PermissionBehavior::Allow) => ctx.require_can_use_tool,
        Some(coco_types::PermissionBehavior::Ask) | None => true,
    };
    if !should_run {
        return None;
    }

    let handle = ctx.can_use_tool.clone()?;
    let cb_ctx = coco_tool_runtime::CanUseToolCallContext {
        tool_use_id: tool_call.tool_call_id.clone(),
        abort: ctx.abort.turn_signal(),
        require_can_use_tool: ctx.require_can_use_tool,
        messages: ctx.messages.clone(),
    };
    match handle
        .check(&tool_call.tool_name, effective_input, &cb_ctx)
        .await
    {
        CanUseToolDecision::Deny {
            message,
            decision_reason,
        } => {
            tracing::info!(
                tool_use_id = %tool_call.tool_call_id,
                tool_name = %tool_call.tool_name,
                decision_reason = ?decision_reason,
                "fork canUseTool denied call"
            );
            Some(CanUseToolResolution::Decision(PermissionDecision::Deny {
                message,
                reason: coco_types::PermissionDecisionReason::AsyncAgent {
                    reason: can_use_tool_reason_label(&decision_reason),
                },
            }))
        }
        CanUseToolDecision::Allow {
            updated_input,
            decision_reason,
        } => {
            tracing::debug!(
                tool_use_id = %tool_call.tool_call_id,
                tool_name = %tool_call.tool_name,
                decision_reason = ?decision_reason,
                updated = updated_input.is_some(),
                "fork canUseTool allowed call"
            );
            Some(CanUseToolResolution::Decision(PermissionDecision::Allow {
                updated_input,
                feedback: Some(can_use_tool_reason_label(&decision_reason)),
            }))
        }
        CanUseToolDecision::Ask { decision_reason } => {
            tracing::debug!(
                tool_use_id = %tool_call.tool_call_id,
                tool_name = %tool_call.tool_name,
                decision_reason = ?decision_reason,
                "fork canUseTool abstained; falling through"
            );
            Some(CanUseToolResolution::Ask)
        }
    }
}

fn can_use_tool_reason_label(reason: &DecisionReason) -> String {
    match reason {
        DecisionReason::Other { reason } => reason.clone(),
        DecisionReason::RuleAllow { rule_kind } => format!("rule_allow:{rule_kind}"),
        DecisionReason::RuleDeny { rule_kind } => format!("rule_deny:{rule_kind}"),
        DecisionReason::ModeAllow => "mode_allow".into(),
        DecisionReason::UserAccept => "user_accept".into(),
        DecisionReason::UserReject => "user_reject".into(),
        DecisionReason::Speculation { boundary } => format!("speculation:{boundary:?}"),
    }
}

/// Run the central rule evaluator against a tool call.
///
/// The tool's own opinion (`Tool::check_permissions`) is captured
/// once and supplied as the step-1c slot to
/// [`coco_permissions::PermissionEvaluator::evaluate_with_tool_check`],
/// so the same `ToolCheckResult` passes through deny rules → tool
/// opinion → allow rules → ask rules → path safety → MCP server
/// rules → mode fallthrough.
///
/// Returning `Allow { updated_input: Some(_) }` from the tool's
/// opinion survives an evaluator-side `Allow` decision — `updatedInput`
/// is preserved on downstream allows so a tool can normalize input
/// even when a user-allow rule is present.
async fn evaluate_with_rules(
    tool: &Arc<dyn DynTool>,
    effective_input: &Value,
    ctx: &ToolUseContext,
) -> PermissionDecision {
    use coco_types::ToolCheckResult;

    let tool_opinion = tool.check_permissions(effective_input, ctx).await;
    let tool_check = move |_id: &ToolId,
                           _input: &Value,
                           _pc: &coco_types::ToolPermissionContext|
          -> ToolCheckResult { tool_opinion.clone() };

    let tool_id = tool.id();
    let shell_cwd = shell_analysis_cwd(&tool_id, ctx).await;
    let dynamic_read_only = dynamic_read_only(
        tool.as_ref(),
        &tool_id,
        effective_input,
        shell_cwd.as_deref(),
    );
    let sandbox_auto_allow_bash = sandbox_auto_allow_bash(&tool_id, effective_input, ctx);
    // `evaluate_with_tool_check` step 1c short-circuits with the
    // tool's own `Allow { updated_input, feedback }` before any rule
    // evaluation, so the returned decision already preserves
    // `updated_input` when the tool returned one. No post-processing
    // needed here.
    coco_permissions::PermissionEvaluator::evaluate_with_tool_check_and_options(
        &tool_id,
        effective_input,
        &ctx.permission_context,
        Some(&tool_check),
        coco_permissions::PermissionEvaluationOptions {
            dynamic_read_only,
            sandbox_auto_allow_bash,
            shell_cwd,
        },
    )
}

async fn shell_analysis_cwd(tool_id: &ToolId, ctx: &ToolUseContext) -> Option<String> {
    if matches!(tool_id, ToolId::Builtin(ToolName::Bash)) {
        Some(
            ctx.effective_shell_cwd()
                .await
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    }
}

/// Whether the auto-mode classifier should run for THIS tool call.
///
/// TS parity (`permissions.ts:520`): the gate is driven by the per-call
/// permission context `mode`, never a shared session-global flag. Primary path
/// is `Auto`; `Plan` bridges to the classifier only when the narrowly-scoped
/// auto flag is set (mirrors `mode === 'plan' && isAutoModeActive()`). Because
/// each subagent carries its own `permission_context.mode`, a concurrent engine
/// build mutating the shared `AutoModeState` can no longer race the classifier
/// off for an unrelated tool call.
fn should_auto_classify(
    mode: coco_types::PermissionMode,
    auto_mode_state: Option<&Arc<coco_permissions::AutoModeState>>,
) -> bool {
    match mode {
        coco_types::PermissionMode::Auto => true,
        coco_types::PermissionMode::Plan => auto_mode_state.is_some_and(|s| s.is_active()),
        _ => false,
    }
}

/// Short tag for a `PermissionDecision`, for the `classifier_outcome` log
/// field (the canonical one in `coco_permissions::evaluate` is private).
fn decision_label(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::Allow { .. } => "allow",
        PermissionDecision::Deny { .. } => "deny",
        PermissionDecision::Ask { .. } => "ask",
        PermissionDecision::Abort { .. } => "abort",
    }
}

fn dynamic_read_only(
    tool: &dyn DynTool,
    tool_id: &ToolId,
    input: &Value,
    shell_cwd: Option<&str>,
) -> bool {
    if matches!(tool_id, ToolId::Builtin(ToolName::Bash)) {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        return bash_dynamic_read_only(command, shell_cwd.unwrap_or("/"));
    }
    tool.is_read_only(input)
}

fn bash_dynamic_read_only(command: &str, cwd: &str) -> bool {
    // Surface *why* a Bash command was judged not-read-only — without this the
    // only signal is `dynamic_read_only=false` at the eval site, which can't
    // distinguish a multi-cwd change, a git-escape, or a statically-unprovable
    // command (a `$var`/`[ test ]`/`for` loop trips the conservative static
    // gate even when semantically read-only). Cap the preview to keep the log
    // line bounded.
    let not_read_only = |reason: &'static str| {
        let preview: String = command.chars().take(120).collect();
        tracing::debug!(reason, command_preview = %preview, "bash not auto-read-only");
        false
    };
    if command.is_empty() {
        return not_read_only("empty_command");
    }
    if coco_shell::check_multiple_cwd_changes(command, cwd).is_some() {
        return not_read_only("multi_cwd_change");
    }
    if coco_shell::has_git_escape_pattern_in_cwd(command, cwd) {
        return not_read_only("git_escape_pattern");
    }
    if coco_shell::read_only::is_read_only_command(command) {
        return true;
    }
    not_read_only("static_not_read_only")
}

async fn dynamic_concurrency_safe(
    tool: &dyn DynTool,
    tool_id: &ToolId,
    input: &Value,
    ctx: &ToolUseContext,
) -> bool {
    if matches!(tool_id, ToolId::Builtin(ToolName::Bash)) {
        let cwd = ctx.effective_shell_cwd().await;
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        return bash_dynamic_read_only(command, &cwd.to_string_lossy());
    }
    tool.is_concurrency_safe(input)
}

/// Returns true iff this is a Bash command that WILL be sandboxed AND
/// `autoAllowBashIfSandboxed` is on. When true, the evaluator skips a
/// tool-wide Bash ask rule and auto-allows.
fn sandbox_auto_allow_bash(tool_id: &ToolId, input: &Value, ctx: &ToolUseContext) -> bool {
    if !matches!(tool_id, ToolId::Builtin(coco_types::ToolName::Bash)) {
        return false;
    }
    // Mirror bash.rs `active_sandbox_state`: feature-gated installed state.
    if !ctx.features.enabled(coco_types::Feature::Sandbox) {
        return false;
    }
    let Some(state) = ctx.sandbox_state.as_ref() else {
        return false;
    };
    // `auto_allow_enabled` folds isSandboxingEnabled && autoAllowBashIfSandboxed.
    if !state.auto_allow_enabled() {
        return false;
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let bypass = coco_sandbox::SandboxBypass::from_flag(
        input
            .get("dangerouslyDisableSandbox")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    );
    // `should_sandbox_command` is the `shouldUseSandbox(input)` mirror.
    state.should_sandbox_command(command, bypass)
}

#[allow(clippy::too_many_arguments)]
async fn try_classify_in_auto_mode<M: std::borrow::Borrow<Message>>(
    tool_name: &str,
    input: &Value,
    is_read_only: bool,
    auto_active: bool,
    tracker: &mut coco_permissions::DenialTracker,
    messages: &[M],
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    auto_mode_rules: &AutoModeRules,
    auto_ctx: coco_permissions::AutoModeContext<'_>,
    tools: &ToolRegistry,
) -> Option<PermissionDecision> {
    let model_runtimes = model_runtimes.clone();
    let classify_fn = move |req: coco_permissions::ClassifyRequest| {
        let model_runtimes = Arc::clone(&model_runtimes);
        async move {
            let prompt: coco_llm_types::LlmPrompt = vec![
                coco_llm_types::LlmMessage::System {
                    content: vec![coco_llm_types::UserContentPart::Text(
                        coco_llm_types::TextPart {
                            text: req.system_prompt,
                            provider_metadata: None,
                        },
                    )],
                    provider_options: None,
                },
                coco_llm_types::LlmMessage::User {
                    content: vec![coco_llm_types::UserContentPart::Text(
                        coco_llm_types::TextPart {
                            text: req.user_prompt,
                            provider_metadata: None,
                        },
                    )],
                    provider_options: None,
                },
            ];
            loop {
                let params = QueryParams {
                    prompt: prompt.clone(),
                    max_tokens: Some(req.max_tokens),
                    thinking_level: None,
                    // The classifier runs on the shared Main runtime. No
                    // priority flag — stage-1 "fastness" comes purely from
                    // the small token budget + `</block>` stop. Toggling
                    // `fast_mode` per stage would only churn the Main
                    // runtime's prompt-cache-break detector.
                    fast_mode: false,
                    tools: None,
                    tool_choice: None,
                    context_management: None,
                    query_source: None,
                    agent_id: None,
                    time_since_last_assistant_ms: None,
                    // Auto-mode classifier helper call — not the agent loop.
                    agentic: false,
                    cache: None,
                    // Stage 1 in `both` mode passes ["</block>"] so the model
                    // terminates immediately after the verdict tag, saving
                    // tokens and latency. Stage 2 leaves this `None` so it can
                    // emit `<thinking>` and `<reason>` freely.
                    stop_sequences: req.stop_sequences.clone(),
                    response_format: None,
                    cancel: None,
                    wire_tap: None,
                };
                match model_runtimes
                    .query_once(ModelRuntimeSource::Role(ModelRole::Main), &params)
                    .await
                {
                    ModelRuntimeQueryOutcome::Success { result, .. } => {
                        // auto-mode classifier input — preserve tool-call
                        // boundary markers so permission decisions see the
                        // structural transitions (otherwise multi-text +
                        // tool calls collapse to a single blob and the
                        // classifier can misclassify).
                        let mut chunks: Vec<String> = Vec::new();
                        for p in &result.content {
                            match p {
                                coco_llm_types::AssistantContentPart::Text(t)
                                    if !t.text.is_empty() =>
                                {
                                    chunks.push(t.text.clone());
                                }
                                coco_llm_types::AssistantContentPart::ToolCall(tc) => {
                                    chunks.push(format!("[tool: {}]", tc.tool_name));
                                }
                                _ => {}
                            }
                        }
                        // Stage 1 uses `["</block>"]` as a stop sequence, so a
                        // `stop_sequence` stop_reason is expected. The danger
                        // is `length` (verdict truncated mid-XML) or
                        // `content-filter` — both yield a structurally
                        // incomplete classifier output that downstream
                        // permission parsing may silently mis-interpret as
                        // "allow". Warn so the permission misroute is
                        // discoverable.
                        let stop = result.stop_reason.as_ref();
                        if stop.is_some_and(coco_messages::FinishReason::is_abnormal)
                            || chunks.is_empty()
                        {
                            tracing::warn!(
                                stop_reason = ?stop,
                                tokens_out = result.usage.output_tokens.total,
                                chunks = chunks.len(),
                                stage = req.stage,
                                "auto-mode classifier unexpected outcome — \
                                 permission decision may use a truncated verdict"
                            );
                        }
                        break Ok(chunks.join("\n"));
                    }
                    ModelRuntimeQueryOutcome::Retry { .. } => continue,
                    ModelRuntimeQueryOutcome::Failed { error, .. } => break Err(error.to_string()),
                }
            }
        }
    };

    // Per-tool projection of the judged action and the prior tool_use blocks
    // in the transcript, resolved from the live registry. `None` from a tool
    // (no projection / unknown) → the classifier falls back to raw JSON; the
    // "no security relevance" auto-allow stays in `is_safe_tool`.
    let projector = |name: &str, value: &Value| {
        tools
            .get_by_name(name)
            .and_then(|t| t.to_auto_classifier_input(value))
    };
    let projector: coco_permissions::InputProjector<'_> = &projector;

    coco_permissions::can_use_tool_in_auto_mode(
        tool_name,
        input,
        is_read_only,
        auto_active,
        tracker,
        messages,
        auto_mode_rules,
        &auto_ctx,
        classify_fn,
        Some(projector),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resolve_effective_input_from_permission(
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    history: &mut MessageHistory,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    tool_id: &ToolId,
    tool: &Arc<dyn DynTool>,
    permission_outcome: PermissionOutcome,
    effective_input: ValidatedInput,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<(
    ValidatedInput,
    Option<String>,
    Option<coco_types::PermissionResolutionDetail>,
)> {
    match permission_outcome {
        PermissionOutcome::Denied => None,
        PermissionOutcome::Aborted => None,
        PermissionOutcome::Allow {
            updated_input,
            approval_feedback,
            resolution_detail,
        } => {
            if let Some(updated_input) = updated_input {
                return validate_effective_input_or_complete_error(
                    event_tx,
                    history,
                    ctx,
                    tool_call,
                    tool_id,
                    tool,
                    updated_input,
                    completion_event_mode,
                    deferred_tool_completions,
                )
                .await
                .map(|input| (input, approval_feedback, resolution_detail));
            }
            Some((effective_input, approval_feedback, resolution_detail))
        }
    }
}

/// Only fires when the decision is a classifier-driven `Deny`. Returns
/// the (possibly rewritten) decision; on `retry: true` we append a hint
/// so the model learns the hook approved the retry.
async fn maybe_fire_permission_denied_hook(
    hook_controller: &HookController<'_>,
    tool_call: &ToolCallPart,
    effective_input: &Value,
    decision: PermissionDecision,
) -> PermissionDecision {
    let PermissionDecision::Deny { message, reason } = decision else {
        return decision;
    };
    let coco_types::PermissionDecisionReason::Classifier {
        reason: classifier_reason,
        ..
    } = &reason
    else {
        return PermissionDecision::Deny { message, reason };
    };

    let retry = hook_controller
        .run_permission_denied(
            &tool_call.tool_name,
            &tool_call.tool_call_id,
            effective_input,
            classifier_reason,
        )
        .await;
    if !retry {
        return PermissionDecision::Deny { message, reason };
    }

    let updated_message = format!(
        "{message}\n\nThe PermissionDenied hook indicated this command is now \
         approved. You may retry it if you would like."
    );
    PermissionDecision::Deny {
        message: updated_message,
        reason,
    }
}

#[allow(clippy::too_many_arguments)]
async fn validate_effective_input_or_complete_error(
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    history: &mut MessageHistory,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    tool_id: &ToolId,
    tool: &Arc<dyn DynTool>,
    input: Value,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<ValidatedInput> {
    let mut deferred_tool_completions = deferred_tool_completions;
    // Coercion + schema validation, fused in the `ValidatedInput`
    // constructor (plan I3 Rust-side tightening): a hook may hand back a
    // freeform tool's raw string (apply_patch's `*** Begin Patch …`
    // envelope) or malformed JSON — both are caught HERE, before
    // `tool.validate_input`, producing a synthetic validation error
    // instead of failing silently downstream.
    //
    // v4.2: the validator is owned by the tool's schema (synchronous,
    // lock-free). A schema-compile failure is impossible here — a tool is
    // only registered if its schema compiled at construction.
    let validated = match ValidatedInput::validate(tool.as_ref(), input) {
        Ok(validated) => validated,
        Err(issues) => {
            let message = format!(
                "Invalid input: {}",
                crate::tool_input_validate::format_schema_error(&tool_call.tool_name, &issues)
            );
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                tool_id,
                &message,
                coco_tool_runtime::ToolCallErrorKind::SchemaFailed,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
    };

    let validation = tool.validate_input(validated.as_value(), ctx);
    if validation.is_valid() {
        return Some(validated);
    }

    let message = match validation {
        coco_tool_runtime::ValidationResult::Invalid { message, .. } => {
            format!("Invalid input: {message}")
        }
        coco_tool_runtime::ValidationResult::Valid => "Invalid input".to_string(),
    };
    complete_tool_call_with_error_mode(
        event_tx,
        history,
        &tool_call.tool_call_id,
        &tool_call.tool_name,
        tool_id,
        &message,
        coco_tool_runtime::ToolCallErrorKind::ValidationFailed,
        completion_event_mode,
        deferred_tool_completions.take(),
    )
    .await;
    None
}

#[cfg(test)]
#[path = "tool_call_preparer.test.rs"]
mod tests;
