//! Elicitation/ElicitationResult hook wrappers around `SendElicitation`.
//!
//! The flow:
//!
//! 1. Receives an MCP `elicit/create` request.
//! 2. Fires `executeElicitationHooks` BEFORE showing the dialog. If the
//!    hook returns an `action` (accept/decline/cancel) the dialog is
//!    skipped and the hook's response goes back to the server.
//! 3. Otherwise shows the dialog, awaits the user's response.
//! 4. Fires `executeElicitationResultHooks` AFTER. The hook can override
//!    the action/content or block (forces decline).
//! 5. Fires `Notification` with `notification_type: "elicitation_response"`.
//!
//! Coco-rs has no dialog UI yet — every `SendElicitation` call site
//! returns `Err`. By wrapping the closure with this helper, hooks fire
//! regardless of whether a UI exists, so users can program-respond to
//! elicitations through hooks even on headless / SDK paths.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use coco_hooks::HookRegistry;
use coco_hooks::orchestration::ElicitationAction;
use coco_hooks::orchestration::ElicitationMode;
use coco_hooks::orchestration::OrchestrationContext;
use coco_types::ElicitationGuard;

/// Build a `SendElicitation` closure that fires `Elicitation` and
/// `ElicitationResult` hooks around the supplied `inner` fallback.
///
/// `inner` is invoked only when the `Elicitation` hook does NOT return
/// a decision (i.e., the dialog only renders when no hook short-circuits).
/// Failures in `inner` propagate as elicitation errors.
///
/// `ctx_factory` produces an `OrchestrationContext` per fire. Each call
/// captures session_id / cwd at the time of firing rather than at
/// closure-creation time so a `/clear` doesn't leave a stale snapshot.
/// `elicitation_counter` (Phase 7 wire-up): clone of
/// `ToolAppState.elicitation_pending_count`. `None` keeps the legacy
/// untracked behaviour for tests / paths without app_state access.
/// When `Some`, every wrapped invocation holds an [`ElicitationGuard`]
/// for the request's full lifetime (pre-hook + dialog/error +
/// post-hook), so the prompt-suggestion fork's
/// `SuppressReason::ElicitationActive` fires correctly.
pub fn wrap_send_elicitation_with_hooks(
    server_name: String,
    registry: Arc<HookRegistry>,
    ctx_factory: Arc<dyn Fn() -> OrchestrationContext + Send + Sync>,
    elicitation_counter: Option<Arc<AtomicU32>>,
    inner: coco_mcp::SendElicitation,
) -> coco_mcp::SendElicitation {
    let inner = std::sync::Arc::new(tokio::sync::Mutex::new(inner));
    Box::new(
        move |request_id,
              elicitation|
              -> Pin<
            Box<
                dyn Future<
                        Output = std::result::Result<
                            coco_mcp::ElicitationResponse,
                            coco_mcp::RmcpClientError,
                        >,
                    > + Send,
            >,
        > {
            let server_name = server_name.clone();
            let registry = registry.clone();
            let ctx_factory = ctx_factory.clone();
            let inner = inner.clone();
            let elicitation_counter = elicitation_counter.clone();
            Box::pin(async move {
                // Phase 7: hold the guard for the entire elicitation
                // lifetime — pre-hook, dialog (or error stub),
                // post-hook. Drop fires when the async block returns,
                // even on `?` early-exit paths, because the guard is
                // moved into this block by value.
                let _elicit_guard = elicitation_counter.map(ElicitationGuard::acquire);
                // rmcp 1.7 made the elicitation request an enum (Form / Url).
                // Form carries a requested_schema; Url carries only a message.
                let (message, requested_schema) = match &elicitation {
                    coco_mcp::Elicitation::FormElicitationParams {
                        message,
                        requested_schema,
                        ..
                    } => (message.clone(), serde_json::to_value(requested_schema).ok()),
                    coco_mcp::Elicitation::UrlElicitationParams { message, .. } => {
                        (message.clone(), None)
                    }
                };

                // Pre-dialog hook.
                let ctx = (ctx_factory)();
                if !ctx.disable_all_hooks {
                    match coco_hooks::orchestration::execute_elicitation(
                        &registry,
                        &ctx,
                        &server_name,
                        &message,
                        Some(ElicitationMode::Form),
                        /*url*/ None,
                        /*elicitation_id*/ None,
                        requested_schema.as_ref(),
                    )
                    .await
                    {
                        Ok(agg) => {
                            // `blockingError` ⇒ decline.
                            if agg.is_blocked() {
                                tracing::debug!(
                                    %server_name,
                                    "Elicitation hook blocked; auto-declining"
                                );
                                return run_result_hook_and(
                                    &registry,
                                    &ctx_factory,
                                    &server_name,
                                    coco_mcp::ElicitationResponse {
                                        action: coco_mcp::RmcpElicitationAction::Decline,
                                        content: None,
                                        meta: None,
                                    },
                                    /*hook_overrode*/ true,
                                )
                                .await;
                            }
                            // Hook returned `action` ⇒ use it as the response.
                            if let Some(resp) = agg.elicitation_response {
                                let response =
                                    build_elicitation_response(resp.action, resp.content);
                                return run_result_hook_and(
                                    &registry,
                                    &ctx_factory,
                                    &server_name,
                                    response,
                                    /*hook_overrode*/ true,
                                )
                                .await;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, %server_name, "Elicitation hook failed; falling through to dialog");
                        }
                    }
                }

                // No hook decision — fall through to the underlying
                // `SendElicitation` (which is a no-op stub today; will
                // become the TUI dialog once that bridge lands).
                let raw_response = {
                    let guard = inner.lock().await;
                    (guard)(request_id, elicitation).await
                }?;

                // Post-dialog hook.
                run_result_hook_and(
                    &registry,
                    &ctx_factory,
                    &server_name,
                    raw_response,
                    /*hook_overrode*/ false,
                )
                .await
            })
        },
    )
}

/// Run `ElicitationResult` hook + emit the `elicitation_response`
/// notification, returning the (potentially overridden) response.
///
/// `hook_overrode` is `true` when the response came from the
/// `Elicitation` hook itself (so the result hook still fires for
/// observability).
async fn run_result_hook_and(
    registry: &Arc<HookRegistry>,
    ctx_factory: &Arc<dyn Fn() -> OrchestrationContext + Send + Sync>,
    server_name: &str,
    response: coco_mcp::ElicitationResponse,
    _hook_overrode: bool,
) -> std::result::Result<coco_mcp::ElicitationResponse, coco_mcp::RmcpClientError> {
    let ctx = (ctx_factory)();
    let action = action_from_rmcp(&response.action);
    let content = response.content.clone();

    if !ctx.disable_all_hooks {
        match coco_hooks::orchestration::execute_elicitation_result(
            registry,
            &ctx,
            server_name,
            /*elicitation_id*/ None,
            Some(ElicitationMode::Form),
            action,
            content.as_ref(),
        )
        .await
        {
            Ok(agg) => {
                let final_response = if agg.is_blocked() {
                    coco_mcp::ElicitationResponse {
                        action: coco_mcp::RmcpElicitationAction::Decline,
                        content: None,
                        meta: None,
                    }
                } else if let Some(override_resp) = agg.elicitation_result_response {
                    build_elicitation_response(
                        override_resp.action,
                        override_resp.content.or(content),
                    )
                } else {
                    response
                };

                // Fire `elicitation_response` Notification at the end.
                let final_action = format!("{:?}", final_response.action).to_lowercase();
                let _ = coco_hooks::orchestration::execute_notification(
                    registry,
                    &ctx,
                    "elicitation_response",
                    &format!("Elicitation response for server \"{server_name}\": {final_action}"),
                    /*title*/ None,
                )
                .await;

                Ok(final_response)
            }
            Err(e) => {
                tracing::warn!(error = %e, "ElicitationResult hook failed");
                let _ = coco_hooks::orchestration::execute_notification(
                    registry,
                    &ctx,
                    "elicitation_response",
                    &format!("Elicitation response for server \"{server_name}\": (hook error)"),
                    /*title*/ None,
                )
                .await;
                Ok(response)
            }
        }
    } else {
        Ok(response)
    }
}

fn action_from_rmcp(action: &coco_mcp::RmcpElicitationAction) -> ElicitationAction {
    match action {
        coco_mcp::RmcpElicitationAction::Accept => ElicitationAction::Accept,
        coco_mcp::RmcpElicitationAction::Decline => ElicitationAction::Decline,
        coco_mcp::RmcpElicitationAction::Cancel => ElicitationAction::Cancel,
    }
}

fn build_elicitation_response(
    action: ElicitationAction,
    content: Option<serde_json::Value>,
) -> coco_mcp::ElicitationResponse {
    let action = match action {
        ElicitationAction::Accept => coco_mcp::RmcpElicitationAction::Accept,
        ElicitationAction::Decline => coco_mcp::RmcpElicitationAction::Decline,
        ElicitationAction::Cancel => coco_mcp::RmcpElicitationAction::Cancel,
    };
    coco_mcp::ElicitationResponse {
        action,
        content,
        meta: None,
    }
}
