//! Turn lifecycle (`turn/*`) plus per-category resolve handlers that
//! drain entries from the pending_* maps back into the awaiting agent
//! task (approval, user input, elicitation, hook callback, mcp route),
//! and the `cancelRequest` handler that evicts pending entries
//! without delivery.

use coco_types::ApprovalResolveParams;
use coco_types::CoreEvent;
use coco_types::ElicitationResolveParams;
use coco_types::TurnStartParams;
use coco_types::UserInputResolveParams;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

use super::HandlerContext;
use super::HandlerResult;
use super::session::forward_turn_events;
use crate::sdk_server::pending_map::ResolveOutcome;

/// `turn/start` — begin a single agent turn in the active session.
///
/// Fire-and-forget: the dispatcher delegates to the configured
/// [`super::TurnRunner`] (spawned on a detached task) and replies
/// immediately with a `turn_id`. Progress flows back via `turn/started`,
/// streaming deltas, and the terminal `turn/ended` notification (whose
/// discriminated `outcome` carries completed / failed / interrupted /
/// max_turns_reached / budget_exhausted) on the shared `notif_tx` channel.
///
/// Errors:
/// - `INVALID_REQUEST` if no session is active.
/// - `INVALID_REQUEST` if a turn is already in flight (one-at-a-time).
///
/// In headless mode a single turn is kicked off per invocation; coco-rs
/// lets the SDK client drive the cadence via `turn/start`.
pub(super) async fn handle_turn_start(
    params: TurnStartParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Read `turn_runner` BEFORE acquiring the session write lock. Lock
    // order is turn_runner → session elsewhere too; avoids nesting a
    // second lock acquisition under the session write guard.
    let runner = ctx.state.turn_runner.read().await.clone();

    // Entire per-turn setup runs under the session write lock so that
    // the spawned forwarder + runner handles are stored on the
    // SessionHandle before the lock is released — `session/archive` can
    // rely on finding them when it cancels and waits for flushing.
    let turn_id = {
        let mut slot = ctx.state.session.write().await;
        let Some(session) = slot.as_mut() else {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no active session; call session/start first".into(),
                data: None,
            };
        };
        if session.active_turn_cancel.is_some() {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "a turn is already running; call turn/interrupt first".into(),
                data: None,
            };
        }
        session.turn_counter = session.turn_counter.saturating_add(1);
        let turn_id = format!("turn-{}-{}", session.session_id, session.turn_counter);
        let cancel_token = CancellationToken::new();
        session.active_turn_cancel = Some(cancel_token.clone());
        // Narrow handoff built under the lock — avoids cloning `stats` /
        // `env_overrides` / `permission_denials` out of the SessionHandle.
        let handoff = session.handoff();

        info!(
            session_id = %handoff.session_id,
            turn_id = %turn_id,
            "SdkServer: turn/start"
        );

        // Event-forwarder bridge: the runner writes to `inner_tx`; the
        // forwarder task reads events, intercepts `SessionResult` to
        // fold per-turn stats into `SessionHandle.stats`, and forwards
        // everything else (sans SessionStarted / SessionResult) to the
        // real `notif_tx`.
        //
        // This decouples the engine's "one SessionResult per
        // run_with_events" assumption from the SDK's "one SessionResult
        // per session" wire contract. See `event-system-design.md`.
        //
        // The forwarder is parameterized by the owner session_id so it
        // can refuse to fold stats into a DIFFERENT session after
        // archive + session/start has replaced the slot.
        let (inner_tx, inner_rx) = mpsc::channel::<CoreEvent>(256);
        let forwarder_handle = tokio::spawn(forward_turn_events(
            inner_rx,
            ctx.notif_tx.clone(),
            ctx.state.clone(),
            handoff.session_id.clone(),
        ));
        session.active_turn_forwarder = Some(forwarder_handle);

        // Spawn the turn as a detached task so `turn/start` returns the
        // turn_id synchronously. The task's post-run cleanup clears its
        // own handle fields only if the session is still the same one
        // (cross-session guard).
        let state = ctx.state.clone();
        let turn_id_for_task = turn_id.clone();
        let owner_session_id = handoff.session_id.clone();
        let turn_handle = tokio::spawn(async move {
            let run_result = runner
                .run_turn(params, handoff, inner_tx, cancel_token)
                .await;
            if let Err(e) = run_result {
                warn!(turn_id = %turn_id_for_task, error = %e, "turn runner failed");
            }
            // Cross-session guard: only clear if the session in the
            // slot is STILL the session this turn belonged to. If
            // `session/archive` + `session/start` ran while this turn
            // was winding down, the slot now holds a different session
            // (or is `None` during archive-in-progress) and we must not
            // touch it.
            let mut slot = state.session.write().await;
            if let Some(session) = slot.as_mut()
                && session.session_id == owner_session_id
            {
                session.active_turn_cancel = None;
                session.active_turn_task = None;
                session.active_turn_forwarder = None;
            }
        });
        session.active_turn_task = Some(turn_handle);

        turn_id
    };

    HandlerResult::ok(coco_types::TurnStartResult { turn_id })
}

/// `turn/interrupt` — cancel the currently-running turn (if any).
///
/// Cancellation is cooperative: the runner's task is notified via the
/// `CancellationToken` it received from `turn/start`. The runner is
/// expected to observe `cancel.is_cancelled()` at tool boundaries and
/// emit a `turn/failed` notification before exiting.
pub(super) async fn handle_turn_interrupt(ctx: &HandlerContext) -> HandlerResult {
    let slot = ctx.state.session.read().await;
    let Some(session) = slot.as_ref() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    match &session.active_turn_cancel {
        Some(token) => {
            info!(session_id = %session.session_id, "SdkServer: turn/interrupt");
            token.cancel();
            HandlerResult::ok_empty()
        }
        None => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no turn in flight to interrupt".into(),
            data: None,
        },
    }
}

/// `approval/resolve` — resolve a pending `approval/askForApproval`
/// ServerRequest with the client's decision.
///
/// The dispatcher holds a map of pending approvals keyed by `request_id`
/// (see [`super::SdkServerState::pending_approvals`]). When the agent's
/// tool executor hits a gate that needs SDK approval, it registers a
/// oneshot via [`super::SdkServerState::register_approval`], sends an
/// `AskForApproval` ServerRequest on the wire, and awaits the receiver.
/// This handler completes the round trip by looking up the sender and
/// delivering the client-supplied `ApprovalResolveParams`.
///
/// Errors:
/// - `INVALID_REQUEST` if `request_id` does not match any pending approval.
///   This usually means the client replied twice or is responding to a
///   stale/cancelled request.
pub(super) async fn handle_approval_resolve(
    params: ApprovalResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    let decision = params.decision;
    let outcome = ctx
        .state
        .pending_approvals
        .resolve(&request_id, params)
        .await;
    handle_resolve_outcome(outcome, "approval", &request_id, |state| {
        info!(request_id = %state, decision = ?decision, "SdkServer: approval/resolve");
    })
}

/// `elicitation/resolve` — resolve a pending MCP elicitation request
/// with the user's form input (or rejection).
///
/// An MCP server sent a `ServerRequest::RequestElicitation` asking for
/// structured input, the agent registered a oneshot via
/// [`super::SdkServerState::register_elicitation`], and this handler
/// wakes the waiting MCP client with the populated form values (or a
/// rejection if `approved=false`).
///
/// Errors:
/// - `INVALID_REQUEST` if `request_id` doesn't match any pending
///   elicitation. Typical causes: duplicate resolve, stale request after
///   a turn cancellation, protocol confusion.
pub(super) async fn handle_elicitation_resolve(
    params: ElicitationResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    let mcp_server = params.mcp_server_name.clone();
    let approved = params.approved;
    let outcome = ctx
        .state
        .pending_elicitations
        .resolve(&request_id, params)
        .await;
    handle_resolve_outcome(outcome, "elicitation", &request_id, |id| {
        info!(
            request_id = %id,
            mcp_server = %mcp_server,
            approved = approved,
            "SdkServer: elicitation/resolve"
        );
    })
}

/// `input/resolveUserInput` — resolve a pending `input/requestUserInput`
/// ServerRequest with the user's answer (free-form or multiple-choice).
pub(super) async fn handle_user_input_resolve(
    params: UserInputResolveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id.clone();
    let outcome = ctx
        .state
        .pending_user_input
        .resolve(&request_id, params)
        .await;
    handle_resolve_outcome(outcome, "user input", &request_id, |id| {
        info!(request_id = %id, "SdkServer: input/resolveUserInput");
    })
}

/// `control/cancelRequest` — cancel a previously-issued ServerRequest.
///
/// The SDK client uses this to abort a `ServerRequest::AskForApproval`
/// (or similar) that it no longer wants to resolve, e.g. if the user
/// closed the approval UI before answering.
///
/// We drop the pending oneshot sender so the agent-side receiver gets
/// an `Err(RecvError)` and the tool executor can treat it as "denied".
/// If the `request_id` isn't in any pending map, we still return ok so
/// the client doesn't treat a race (server already resolved + cleaned
/// up) as a protocol error.
pub(super) async fn handle_cancel_request(
    params: coco_types::CancelRequestParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let request_id = params.request_id;
    let reason = params.reason.as_deref().unwrap_or("(no reason given)");
    // Try every pending map; the id is unique across categories so at most
    // one hits. Hook and MCP-route requests are no longer pending-tracked
    // — they ride the synchronous JSON-RPC reply path on `pending_server_requests`
    // and the outer dispatcher cancels via that path.
    let cancelled_kind = if ctx.state.pending_approvals.remove(&request_id).await {
        Some("approval")
    } else if ctx.state.pending_user_input.remove(&request_id).await {
        Some("user_input")
    } else if ctx.state.pending_elicitations.remove(&request_id).await {
        Some("elicitation")
    } else {
        None
    };
    match cancelled_kind {
        Some(kind) => info!(
            request_id = %request_id,
            reason = %reason,
            kind = kind,
            "SdkServer: control/cancelRequest"
        ),
        None => info!(
            request_id = %request_id,
            reason = %reason,
            "SdkServer: control/cancelRequest — no pending request matched (already resolved?)"
        ),
    }
    HandlerResult::ok_empty()
}

/// Translate a `ResolveOutcome` into a `HandlerResult` with consistent
/// logging across every `*_resolve` handler.
///
/// `kind` is a short tag (e.g. "approval", "elicitation") used in the error
/// message and the receiver-dropped warning. `on_delivered` emits the
/// happy-path structured log at info level; it runs with the request id
/// only when the payload was actually handed to a live receiver.
fn handle_resolve_outcome(
    outcome: ResolveOutcome,
    kind: &str,
    request_id: &str,
    on_delivered: impl FnOnce(&str),
) -> HandlerResult {
    match outcome {
        ResolveOutcome::Delivered => {
            on_delivered(request_id);
            HandlerResult::ok_empty()
        }
        ResolveOutcome::ReceiverDropped => {
            // Agent-side awaiter has been dropped (e.g. the turn was
            // cancelled mid-request). Still acknowledge so the client
            // doesn't hang.
            warn!(
                request_id = %request_id,
                kind = kind,
                "resolve: agent receiver dropped before delivery"
            );
            HandlerResult::ok_empty()
        }
        ResolveOutcome::NotFound => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("no pending {kind} with request_id {request_id}"),
            data: None,
        },
    }
}
