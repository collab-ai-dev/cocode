use coco_types::CoreEvent;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{
    ActiveTurnHandles, ActiveTurnStartError, ActiveTurnStartState, HandlerContext, HandlerResult,
    ShortcutTurnState, session, session::forward_turn_events, turn,
};
use crate::app_server_host::outbound::{OutboundMessage, send_session_event};
use crate::session_controls::{self, SessionControlError};
use crate::session_memory::{self, SessionMemoryRefresh, SessionMemoryRefreshResult};

enum TurnStartObservabilityShortcut {
    Cost,
    Status,
}

enum TurnStartGoalShortcut {
    Complete(HandlerResult),
    RunWithPrompt(String),
}

pub(super) enum TurnStartShortcut {
    Complete(HandlerResult),
    RunWithPrompt(String),
    NotShortcut,
}

impl TurnStartObservabilityShortcut {
    fn parse(prompt: &str) -> Option<Self> {
        if coco_commands::handlers::cost::parse_cost_sentinel(prompt).is_some() {
            return Some(Self::Cost);
        }
        if coco_commands::parse_status_sentinel(prompt).is_some() {
            return Some(Self::Status);
        }
        None
    }
}

impl SessionMemoryRefresh {
    fn parse_turn_start_sentinel(prompt: &str) -> Option<Self> {
        if coco_commands::handlers::dream::parse_dream_sentinel(prompt).is_some() {
            return Some(Self::Dream);
        }
        if coco_commands::handlers::summary::parse_summary_sentinel(prompt).is_some() {
            return Some(Self::Summary);
        }
        None
    }
}

pub(super) async fn handle_turn_start_shortcut(
    prompt: &str,
    ctx: &HandlerContext,
) -> TurnStartShortcut {
    if let Some(rename) = coco_commands::parse_rename_sentinel(prompt) {
        return TurnStartShortcut::Complete(handle_turn_start_rename_shortcut(rename, ctx).await);
    }
    if let Some(shortcut) = SessionMemoryRefresh::parse_turn_start_sentinel(prompt) {
        return TurnStartShortcut::Complete(handle_turn_start_memory_shortcut(shortcut, ctx).await);
    }
    if let Some(request) = coco_commands::handlers::btw::parse_btw_sentinel(prompt) {
        return TurnStartShortcut::Complete(handle_turn_start_btw_shortcut(request, ctx).await);
    }
    if let Some(request) = coco_commands::handlers::compact::parse_compact_sentinel(prompt) {
        return TurnStartShortcut::Complete(handle_turn_start_compact_shortcut(request, ctx).await);
    }
    if let Some(request) = coco_commands::parse_goal_sentinel(prompt) {
        return match handle_turn_start_goal_shortcut(request, ctx).await {
            Ok(TurnStartGoalShortcut::Complete(result)) => TurnStartShortcut::Complete(result),
            Ok(TurnStartGoalShortcut::RunWithPrompt(prompt)) => {
                TurnStartShortcut::RunWithPrompt(prompt)
            }
            Err(error) => TurnStartShortcut::Complete(error),
        };
    }
    if let Some(shortcut) = TurnStartObservabilityShortcut::parse(prompt) {
        return TurnStartShortcut::Complete(
            handle_turn_start_observability_shortcut(shortcut, ctx).await,
        );
    }
    TurnStartShortcut::NotShortcut
}

async fn handle_turn_start_observability_shortcut(
    shortcut: TurnStartObservabilityShortcut,
    ctx: &HandlerContext,
) -> HandlerResult {
    let shortcut_turn = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn,
        Err(error) => return error,
    };

    let text = match shortcut {
        TurnStartObservabilityShortcut::Cost => {
            match session_controls::session_cost(Some(shortcut_turn.session.clone())).await {
                Ok(result) => result.text,
                Err(error) => return session_control_error(error),
            }
        }
        TurnStartObservabilityShortcut::Status => {
            match session_controls::session_status(Some(shortcut_turn.session.clone())).await {
                Ok(result) => result.text,
                Err(error) => return session_control_error(error),
            }
        }
    };
    crate::session_messages::append_meta_text_to_history(&shortcut_turn.session, &text).await;
    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, Some(text)).await;

    HandlerResult::ok(coco_types::TurnStartResult {
        turn_id: shortcut_turn.turn_id,
    })
}

async fn handle_turn_start_rename_shortcut(
    rename: coco_commands::ParsedRename,
    ctx: &HandlerContext,
) -> HandlerResult {
    let shortcut_turn = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn,
        Err(error) => return error,
    };
    let turn_id = shortcut_turn.turn_id.clone();

    if coco_coordinator::identity::is_teammate() {
        warn!("AppServerHost rename ignored: session is a swarm teammate");
        emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, None).await;
        return HandlerResult::ok(coco_types::TurnStartResult { turn_id });
    }

    let name = match crate::session_labels::resolve_rename_name(
        Some(&shortcut_turn.session),
        rename,
    )
    .await
    {
        Ok(name) => name,
        Err(error) => {
            warn!(reason = %error.user_message(), "AppServerHost rename resolution failed");
            emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, None).await;
            return HandlerResult::ok(coco_types::TurnStartResult { turn_id });
        }
    };

    let Some(session_id) = ctx.target_session_id.clone() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session rename requires an explicitly targeted session".to_string(),
            data: None,
        };
    };
    let rename_result_text = format!("Renamed session to {name}");
    match session::handle_session_rename(
        coco_types::SessionRenameParams {
            target: coco_types::SessionTarget { session_id },
            name,
        },
        ctx,
    )
    .await
    {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err { message, .. } => {
            warn!(error = %message, "AppServerHost rename persist failed");
        }
        HandlerResult::NotImplemented(method) => {
            warn!(method = %method, "AppServerHost rename persist failed: handler not implemented");
        }
    }
    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, Some(rename_result_text)).await;

    HandlerResult::ok(coco_types::TurnStartResult { turn_id })
}

async fn handle_turn_start_memory_shortcut(
    shortcut: SessionMemoryRefresh,
    ctx: &HandlerContext,
) -> HandlerResult {
    let shortcut_turn = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn,
        Err(error) => return error,
    };

    match session_memory::refresh_memory(ctx.resolve_runtime().await, shortcut).await {
        SessionMemoryRefreshResult::Ran => {}
        SessionMemoryRefreshResult::NoRuntime | SessionMemoryRefreshResult::NoMemoryRuntime => {
            match shortcut {
                SessionMemoryRefresh::Dream => {
                    info!(
                        "AppServerHost /dream: no MemoryRuntime (Feature::AutoMemory off); skipping"
                    );
                }
                SessionMemoryRefresh::Summary => {
                    info!("AppServerHost /summary: no MemoryRuntime; skipping");
                }
            }
        }
    }

    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, None).await;
    HandlerResult::ok(coco_types::TurnStartResult {
        turn_id: shortcut_turn.turn_id,
    })
}

async fn handle_turn_start_btw_shortcut(
    request: coco_commands::handlers::btw::BtwRequest,
    ctx: &HandlerContext,
) -> HandlerResult {
    let shortcut_turn = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn,
        Err(error) => return error,
    };

    let response_text = crate::side_question::run_side_question_for_session(
        ctx.resolve_runtime().await,
        &request.question,
    )
    .await;

    let messages = crate::session_messages::append_slash_text_to_history(
        &shortcut_turn.session,
        "btw",
        &request.question,
        &response_text,
        /*is_sensitive*/ false,
    )
    .await;
    send_appended_messages(&ctx.notif_tx, shortcut_turn.session_id.clone(), messages).await;
    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, Some(response_text)).await;

    HandlerResult::ok(coco_types::TurnStartResult {
        turn_id: shortcut_turn.turn_id,
    })
}

async fn handle_turn_start_goal_shortcut(
    request: coco_commands::GoalCommandRequest,
    ctx: &HandlerContext,
) -> Result<TurnStartGoalShortcut, HandlerResult> {
    let runtime_handle = match ctx.resolve_runtime().await {
        Some(runtime) => runtime,
        None => {
            return Err(HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no session runtime installed; /goal requires CLI bootstrap".into(),
                data: None,
            });
        }
    };
    if runtime_handle.has_active_turn() {
        return Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "a turn is already running; call turn/interrupt first".into(),
            data: None,
        });
    }
    let args = crate::goal_command::goal_display_args(&request).to_string();
    let outcome =
        crate::goal_command::resolve_goal_request_for_session(&runtime_handle, request, false)
            .await;
    match outcome {
        crate::goal_command::GoalOutcome::Text(text) => {
            let shortcut_turn = mint_shortcut_turn(ctx).await?;
            append_slash_text_and_emit(&runtime_handle, &ctx.notif_tx, "goal", &args, &text).await;
            emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, Some(text)).await;
            Ok(TurnStartGoalShortcut::Complete(HandlerResult::ok(
                coco_types::TurnStartResult {
                    turn_id: shortcut_turn.turn_id,
                },
            )))
        }
        crate::goal_command::GoalOutcome::StatusThenText { status, text } => {
            let shortcut_turn = mint_shortcut_turn(ctx).await?;
            append_goal_status_and_slash_text_and_emit(
                &runtime_handle,
                &ctx.notif_tx,
                status,
                &args,
                &text,
            )
            .await;
            emit_active_goal_snapshot(&runtime_handle, &ctx.notif_tx).await;
            emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx, Some(text)).await;
            Ok(TurnStartGoalShortcut::Complete(HandlerResult::ok(
                coco_types::TurnStartResult {
                    turn_id: shortcut_turn.turn_id,
                },
            )))
        }
        crate::goal_command::GoalOutcome::SetAndRun {
            status,
            text,
            kickoff,
        } => {
            append_goal_status_and_emit(&runtime_handle, &ctx.notif_tx, status).await;
            emit_active_goal_snapshot(&runtime_handle, &ctx.notif_tx).await;
            append_slash_text_and_emit(&runtime_handle, &ctx.notif_tx, "goal", &args, &text).await;
            Ok(TurnStartGoalShortcut::RunWithPrompt(kickoff))
        }
    }
}

async fn handle_turn_start_compact_shortcut(
    request: coco_commands::handlers::compact::CompactRequest,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime = match ctx.resolve_runtime().await {
        Some(runtime) => runtime,
        None => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: "no session runtime installed; /compact requires CLI bootstrap".into(),
                data: None,
            };
        }
    };

    let notif_tx = ctx.notif_tx.clone();
    let runtime_for_start = runtime.clone();
    let Some(turn_session_id) = ctx.active_session_id().await else {
        return turn::active_turn_start_error(ActiveTurnStartError::NoActiveSession);
    };
    let turn_id = match turn::start_active_turn_for_runtime(
        &runtime,
        turn_session_id,
        move |start: ActiveTurnStartState| {
            let (inner_tx, inner_rx) = mpsc::channel::<CoreEvent>(256);
            let forwarder_handle = tokio::spawn(forward_turn_events(
                inner_rx,
                notif_tx,
                runtime_for_start.clone(),
                start.session_id.clone(),
            ));

            let turn_id_for_task = start.turn_id.clone();
            let turn_id_for_log = turn_id_for_task.clone();
            let cancel_token_for_task = start.cancel_token.clone();
            let runtime_for_task = runtime_for_start.clone();
            let turn_handle = tokio::spawn(async move {
                crate::session_compaction::run_manual_compact_turn(
                    runtime_for_task,
                    request,
                    turn_id_for_task.clone(),
                    inner_tx,
                    cancel_token_for_task,
                )
                .await;
            });

            info!(turn_id = %turn_id_for_log, "AppServerHost: /compact shortcut");
            ActiveTurnHandles {
                cancel_token: start.cancel_token,
                turn_task: turn_handle,
                forwarder_task: forwarder_handle,
            }
        },
    ) {
        Ok(turn_id) => turn_id,
        Err(error) => return turn::active_turn_start_error(error),
    };

    HandlerResult::ok(coco_types::TurnStartResult { turn_id })
}

async fn append_slash_text_and_emit(
    session: &crate::session_runtime::SessionHandle,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    command: &str,
    args: &str,
    text: &str,
) {
    let session_id = session.session_id().clone();
    let messages = crate::session_messages::append_slash_text_to_history(
        session, command, args, text, /*is_sensitive*/ false,
    )
    .await;
    send_appended_messages(notif_tx, session_id, messages).await;
}

async fn append_goal_status_and_emit(
    session: &crate::session_runtime::SessionHandle,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    payload: coco_types::GoalStatusPayload,
) {
    let session_id = session.session_id().clone();
    let messages = crate::goal_command::append_goal_status_to_history(session, payload).await;
    send_appended_messages(notif_tx, session_id, messages).await;
}

async fn append_goal_status_and_slash_text_and_emit(
    session: &crate::session_runtime::SessionHandle,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let session_id = session.session_id().clone();
    let messages =
        crate::goal_command::append_goal_status_and_slash_to_history(session, payload, args, text)
            .await;
    send_appended_messages(notif_tx, session_id, messages).await;
}

async fn emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    notif_tx: &mpsc::Sender<OutboundMessage>,
) {
    let goal = crate::goal_command::persist_active_goal_snapshot(session).await;
    let _ = send_session_event(
        notif_tx,
        session.session_id().clone(),
        CoreEvent::Protocol(crate::goal_command::active_goal_changed_notification(
            goal.clone(),
        )),
    )
    .await;
}

async fn send_appended_messages(
    notif_tx: &mpsc::Sender<OutboundMessage>,
    session_id: coco_types::SessionId,
    messages: Vec<std::sync::Arc<coco_messages::Message>>,
) {
    for message in messages {
        let _ = send_session_event(
            notif_tx,
            session_id.clone(),
            CoreEvent::Protocol(coco_types::ServerNotification::MessageAppended {
                message,
                identity: coco_types::ServerNotificationIdentity::default(),
            }),
        )
        .await;
    }
}

async fn emit_shortcut_turn_lifecycle(
    shortcut_turn: &ShortcutTurnState,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    result_text: Option<String>,
) {
    let turn_id = shortcut_turn.turn_id.clone();
    let result = shortcut_session_result(shortcut_turn, result_text);
    shortcut_turn.session.accumulate_session_result(&result);
    let _ = send_session_event(
        notif_tx,
        shortcut_turn.session_id.clone(),
        CoreEvent::Protocol(coco_types::ServerNotification::TurnStarted(
            coco_types::TurnStartedParams {
                turn_id: turn_id.clone(),
            },
        )),
    )
    .await;
    let _ = send_session_event(
        notif_tx,
        shortcut_turn.session_id.clone(),
        CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(
            coco_types::TurnEndedParams::completed(
                turn_id,
                Some(coco_types::TokenUsage::default()),
                None,
            )
            .with_session_result(result),
        )),
    )
    .await;
}

fn shortcut_session_result(
    shortcut_turn: &ShortcutTurnState,
    result_text: Option<String>,
) -> coco_types::SessionResultParams {
    coco_types::SessionResultParams {
        session_id: shortcut_turn.session_id.clone(),
        total_turns: 1,
        duration_ms: 0,
        duration_api_ms: 0,
        is_error: false,
        stop_reason: "shortcut_completed".to_string(),
        total_cost_usd: 0.0,
        usage: coco_types::TokenUsage::default(),
        model_usage: std::collections::HashMap::new(),
        permission_denials: Vec::new(),
        result: result_text,
        errors: Vec::new(),
        structured_output: None,
        fast_mode_state: None,
        num_api_calls: None,
    }
}

async fn mint_shortcut_turn(ctx: &HandlerContext) -> Result<ShortcutTurnState, HandlerResult> {
    let runtime = ctx
        .resolve_runtime()
        .await
        .ok_or(ActiveTurnStartError::NoActiveSession)
        .map_err(turn::active_turn_start_error)?;
    if runtime.has_active_turn() {
        return Err(turn::active_turn_start_error(
            ActiveTurnStartError::TurnAlreadyRunning,
        ));
    }
    let session_id = runtime.session_id().clone();
    let turn_id = runtime.next_turn_id();
    Ok(ShortcutTurnState {
        session_id,
        turn_id,
        session: runtime,
    })
}

fn session_control_error(error: SessionControlError) -> HandlerResult {
    HandlerResult::Err {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.to_string(),
        data: None,
    }
}
