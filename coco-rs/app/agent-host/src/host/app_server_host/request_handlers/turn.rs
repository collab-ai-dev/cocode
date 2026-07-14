//! Turn lifecycle (`turn/*`) plus per-category resolve handlers that
//! drain pending client requests back into the awaiting agent
//! task (approval, user input, elicitation, hook callback, mcp route),
//! and the `cancelRequest` handler that evicts pending entries
//! without delivery.

use coco_types::{
    ApprovalResolveParams, CoreEvent, ElicitationResolveParams, TurnStartParams,
    UserInputResolveParams,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{
    ActiveTurnHandles, ActiveTurnStartError, ActiveTurnStartState, HandlerContext, HandlerResult,
    session::forward_turn_events,
    turn_shortcuts::{TurnStartShortcut, handle_turn_start_shortcut},
};
use crate::session_controls::{self, SessionControlError};

/// `turn/start` — begin a single agent turn in the active session.
///
/// Fire-and-forget: the dispatcher delegates to the configured
/// [`crate::app_server_host::TurnRunner`](spawned on a detached task) and replies
/// immediately with a `turn_id`. Progress flows back via `turn/started`,
/// streaming deltas, and the terminal `turn/ended` notification (whose
/// discriminated `outcome` carries completed / failed / interrupted /
/// max_turns_reached / budget_exhausted) on the shared `notif_tx` channel.
///
/// Errors:
/// - `INVALID_REQUEST` if no session is active.
/// - `INVALID_REQUEST` if a turn is already in flight (one-at-a-time).
///
/// In headless mode a single turn is kicked off per invocation; AppServer
/// clients drive cadence with repeated `turn/start` calls.
pub(crate) async fn handle_turn_start(
    mut params: TurnStartParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match handle_turn_start_shortcut(&params.prompt, ctx).await {
        TurnStartShortcut::Complete(result) => return result,
        TurnStartShortcut::RunWithPrompt(prompt) => {
            params.prompt = prompt;
        }
        TurnStartShortcut::NotShortcut => {}
    }

    let runner = ctx.state.turn_runner_snapshot().await;
    let Some(session) = ctx.resolve_runtime().await else {
        return active_turn_start_error(ActiveTurnStartError::NoActiveSession);
    };
    let Some(app_server) = ctx.app_server.clone() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "turn/start requires AppServer callback routing".to_string(),
            data: None,
        };
    };
    let Some(turn_session_id) = ctx.active_session_id().await else {
        return active_turn_start_error(ActiveTurnStartError::NoActiveSession);
    };

    let notif_tx = ctx.notif_tx.clone();
    let session_for_start = session.clone();
    let turn_session_id_for_error = turn_session_id.clone();
    let turn_id = match start_active_turn_for_runtime(
        &session,
        turn_session_id,
        move |start: ActiveTurnStartState| {
            info!(
                session_id = %start.session_id,
                turn_id = %start.turn_id,
                "AppServerHost: turn/start"
            );

            // Event-forwarder bridge: the runner writes to `inner_tx`; the
            // forwarder task reads events, intercepts `SessionResult` to
            // fold per-turn stats into runtime-owned accounting, and forwards
            // everything else (sans SessionStarted / SessionResult) to the
            // real `notif_tx`.
            //
            // This decouples the engine's "one SessionResult per
            // run_with_events" assumption from the AppServer client's "one
            // SessionResult per session" wire contract. See
            // `event-system-design.md`.
            //
            let (inner_tx, inner_rx) = mpsc::channel::<CoreEvent>(256);
            let forwarder_handle = tokio::spawn(forward_turn_events(
                inner_rx,
                notif_tx,
                session_for_start.clone(),
                start.session_id.clone(),
            ));

            // Spawn the turn as a detached task so `turn/start` returns the
            // turn_id synchronously. The active-turn record is cleared by the
            // forwarder as it forwards the terminal `TurnEnded`, not here
            // — clearing it in this task raced the forwarder and could wipe a
            // fast next turn's freshly-installed handles.
            let turn_id_for_task = start.turn_id.clone();
            let session_for_task = session_for_start.clone();
            let app_server_for_task = Arc::clone(&app_server);
            let inner_tx_for_error = inner_tx.clone();
            let cancel_token_for_task = start.cancel_token.clone();
            let turn_handle = tokio::spawn(async move {
                let run_result = runner
                    .run_turn(
                        session_for_task,
                        app_server_for_task,
                        params,
                        turn_id_for_task.clone(),
                        inner_tx,
                        cancel_token_for_task,
                    )
                    .await;
                if let Err(e) = run_result {
                    warn!(
                        session_id = %turn_session_id_for_error,
                        turn_id = %turn_id_for_task,
                        error = %e,
                        "turn runner failed"
                    );
                    let _ = inner_tx_for_error
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::TurnEnded(
                                coco_types::TurnEndedParams::failed(
                                    turn_id_for_task.clone(),
                                    /*usage*/ None,
                                    coco_types::ErrorPayload {
                                        message: e.to_string(),
                                        code: coco_types::ErrorCode::Unknown,
                                    },
                                ),
                            ),
                        ))
                        .await;
                }
            });
            ActiveTurnHandles {
                cancel_token: start.cancel_token,
                turn_task: turn_handle,
                forwarder_task: forwarder_handle,
            }
        },
    ) {
        Ok(turn_id) => turn_id,
        Err(error) => return active_turn_start_error(error),
    };

    HandlerResult::ok(coco_types::TurnStartResult { turn_id })
}

pub(super) fn start_active_turn_for_runtime(
    session: &crate::session_runtime::SessionHandle,
    session_id: coco_types::SessionId,
    build: impl FnOnce(ActiveTurnStartState) -> ActiveTurnHandles,
) -> Result<coco_types::TurnId, ActiveTurnStartError> {
    session
        .start_active_turn(move |turn_id, cancel_token| {
            build(ActiveTurnStartState {
                session_id,
                turn_id,
                cancel_token,
            })
        })
        .map_err(|()| ActiveTurnStartError::TurnAlreadyRunning)
}

pub(super) fn active_turn_start_error(error: ActiveTurnStartError) -> HandlerResult {
    match error {
        ActiveTurnStartError::NoActiveSession => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session; call session/start first".into(),
            data: None,
        },
        ActiveTurnStartError::TurnAlreadyRunning => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "a turn is already running; call turn/interrupt first".into(),
            data: None,
        },
    }
}

fn session_control_error(error: SessionControlError) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.to_string(),
        data: None,
    }
}

/// `turn/interrupt` — cancel the currently-running turn (if any).
///
/// Cancellation is cooperative: the runner's task is notified via the
/// `CancellationToken` it received from `turn/start`. The runner is
/// expected to observe `cancel.is_cancelled()` at tool boundaries and
/// emit a `turn/failed` notification before exiting.
pub(crate) async fn handle_turn_interrupt(ctx: &HandlerContext) -> HandlerResult {
    match session_controls::interrupt_active_turn(ctx.resolve_runtime().await).await {
        Ok(result) => {
            info!(session_id = %result.session_id, "AppServerHost: turn/interrupt");
            HandlerResult::ok_empty()
        }
        Err(SessionControlError::NoActiveTurn) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no turn in flight to interrupt".into(),
            data: None,
        },
        Err(error) => session_control_error(error),
    }
}

/// `approval/resolve` — resolve a pending `approval/askForApproval`
/// ServerRequest with the client's decision.
///
/// The dispatcher holds pending approvals keyed by `request_id`. When the agent's
/// tool executor hits a gate that needs remote approval, AppServer registers a
/// pending server request, sends an `AskForApproval` request on the wire, and
/// awaits the receiver.
/// This handler completes the round trip by looking up the sender and
/// delivering the client-supplied `ApprovalResolveParams`.
///
/// Errors:
/// - `INVALID_REQUEST` if `request_id` does not match any pending approval.
///   This usually means the client replied twice or is responding to a
///   stale/cancelled request.
pub(crate) async fn handle_approval_resolve(
    params: ApprovalResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    let decision = params.decision;
    match resolve_app_server_request(
        ctx,
        coco_app_server::ServerRequestReply::Approval(params),
        "approval",
    ) {
        Ok(()) => {
            info!(request_id = %request_id, decision = ?decision, "AppServerHost: approval/resolve");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
}

/// `elicitation/resolve` — resolve a pending MCP elicitation request
/// with the user's form input (or rejection).
///
/// An MCP server sent a `ServerRequest::RequestElicitation` asking for
/// structured input; AppServer owns the pending request correlation, and this
/// handler wakes the waiting MCP client with the populated form values (or a
/// rejection if `approved=false`).
///
/// Errors:
/// - `INVALID_REQUEST` if `request_id` doesn't match any pending
///   elicitation. Typical causes: duplicate resolve, stale request after
///   a turn cancellation, protocol confusion.
pub(crate) async fn handle_elicitation_resolve(
    params: ElicitationResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    let mcp_server = params.mcp_server_name.clone();
    let approved = params.approved;
    match resolve_app_server_request(
        ctx,
        coco_app_server::ServerRequestReply::Elicitation(params),
        "elicitation",
    ) {
        Ok(()) => {
            info!(request_id = %request_id, mcp_server = %mcp_server, approved, "AppServerHost: elicitation/resolve");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
}

/// `input/resolveUserInput` — resolve a pending `input/requestUserInput`
/// ServerRequest with the user's answer (free-form or multiple-choice).
pub(crate) async fn handle_user_input_resolve(
    params: UserInputResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    match resolve_app_server_request(
        ctx,
        coco_app_server::ServerRequestReply::UserInput(params),
        "user input",
    ) {
        Ok(()) => {
            info!(request_id = %request_id, "AppServerHost: input/resolveUserInput");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
}

/// `control/cancelRequest` — cancel a previously-issued ServerRequest.
///
/// AppServer clients use this to abort a `ServerRequest::AskForApproval`
/// (or similar) that it no longer wants to resolve, e.g. if the user
/// closed the approval UI before answering.
///
/// We drop the pending oneshot sender so the agent-side receiver gets
/// an `Err (RecvError)` and the tool executor can treat it as "denied".
/// If the `request_id` isn't in any pending map, we still return ok so
/// the client doesn't treat a race (server already resolved + cleaned
/// up) as a protocol error.
pub(crate) async fn handle_cancel_request(
    params: coco_types::CancelRequestParams,
    _ctx: &HandlerContext,
) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: format!(
            "control/cancelRequest {} requires AppServer connection routing",
            params.request_id
        ),
        data: None,
    }
}

/// Translate a `ResolveOutcome` into a `HandlerResult` with consistent
/// logging across every `*_resolve` handler.
///
/// `kind` is a short tag (e.g. "approval", "elicitation") used in the error
/// message and the receiver-dropped warning. `on_delivered` emits the
/// happy-path structured log at info level; it runs with the request id
/// only when the payload was actually handed to a live receiver.
fn resolve_app_server_request(
    ctx: &HandlerContext,
    reply: coco_app_server::ServerRequestReply,
    kind: &str,
) -> Result<(), HandlerResult> {
    let request_id = reply.request_id().to_string();
    let Some(app_server) = &ctx.app_server else {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("{kind} resolve requires AppServer routing"),
            data: None,
        });
    };
    let Some(_session_id) = &ctx.target_session_id else {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("{kind} resolve requires an interactive target"),
            data: None,
        });
    };
    let Some(target) = reply.interactive_target().cloned() else {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("{kind} resolve requires an interactive reply target"),
            data: None,
        });
    };
    app_server
        .resolve_server_request(&target, reply)
        .map(|_| ())
        .map_err(|error| {
            let error = crate::app_server_host::session_errors::app_server_lifecycle_error(
                "resolve pending server request",
                error,
            );
            HandlerResult::Err {
                code: error.code,
                message: format!(
                    "cannot resolve pending {kind} request {request_id}: {}",
                    error.message
                ),
                data: error.data,
            }
        })
}
