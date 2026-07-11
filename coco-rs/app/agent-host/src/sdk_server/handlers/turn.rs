//! Turn lifecycle (`turn/*`) plus per-category resolve handlers that
//! drain pending client requests back into the awaiting agent
//! task (approval, user input, elicitation, hook callback, mcp route),
//! and the `cancelRequest` handler that evicts pending entries
//! without delivery.

use coco_types::{
    ApprovalResolveParams, CoreEvent, ElicitationResolveParams, TurnId, TurnStartParams,
    UserInputResolveParams,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{
    ActiveTurnHandles, ActiveTurnStartError, ActiveTurnStartState, HandlerContext, HandlerResult,
    ShortcutTurnState, runtime, session, session::forward_turn_events,
};
use crate::sdk_server::outbound::{OutboundMessage, send_session_event};

/// `turn/start` — begin a single agent turn in the active session.
///
/// Fire-and-forget: the dispatcher delegates to the configured
/// [`super::TurnRunner`](spawned on a detached task) and replies
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
    mut params: TurnStartParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    if let Some(rename) = coco_commands::parse_rename_sentinel(&params.prompt) {
        return handle_turn_start_rename_shortcut(rename, ctx).await;
    }
    if let Some(shortcut) = TurnStartMemoryShortcut::parse(&params.prompt) {
        return handle_turn_start_memory_shortcut(shortcut, ctx).await;
    }
    if let Some(request) = coco_commands::handlers::btw::parse_btw_sentinel(&params.prompt) {
        return handle_turn_start_btw_shortcut(request, ctx).await;
    }
    if let Some(request) = coco_commands::handlers::compact::parse_compact_sentinel(&params.prompt)
    {
        return handle_turn_start_compact_shortcut(request, ctx).await;
    }
    if let Some(request) = coco_commands::parse_goal_sentinel(&params.prompt) {
        match handle_turn_start_goal_shortcut(request, ctx).await {
            Ok(TurnStartGoalShortcut::Complete(result)) => return result,
            Ok(TurnStartGoalShortcut::RunWithPrompt(prompt)) => {
                params.prompt = prompt;
            }
            Err(error) => return error,
        }
    }
    if let Some(shortcut) = TurnStartObservabilityShortcut::parse(&params.prompt) {
        return handle_turn_start_observability_shortcut(shortcut, ctx).await;
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
                "SdkServer: turn/start"
            );

            // Event-forwarder bridge: the runner writes to `inner_tx`; the
            // forwarder task reads events, intercepts `SessionResult` to
            // fold per-turn stats into runtime-owned accounting, and forwards
            // everything else (sans SessionStarted / SessionResult) to the
            // real `notif_tx`.
            //
            // This decouples the engine's "one SessionResult per
            // run_with_events" assumption from the SDK's "one SessionResult
            // per session" wire contract. See `event-system-design.md`.
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

enum TurnStartObservabilityShortcut {
    Cost,
    Status,
}

enum TurnStartMemoryShortcut {
    Dream,
    Summary,
}

enum TurnStartGoalShortcut {
    Complete(HandlerResult),
    RunWithPrompt(String),
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

impl TurnStartMemoryShortcut {
    fn parse(prompt: &str) -> Option<Self> {
        if coco_commands::handlers::dream::parse_dream_sentinel(prompt).is_some() {
            return Some(Self::Dream);
        }
        if coco_commands::handlers::summary::parse_summary_sentinel(prompt).is_some() {
            return Some(Self::Summary);
        }
        None
    }
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
            match decode_session_cost_text(runtime::handle_session_cost(ctx).await) {
                Ok(text) => text,
                Err(error) => return error,
            }
        }
        TurnStartObservabilityShortcut::Status => {
            match decode_session_status_text(runtime::handle_session_status(ctx).await) {
                Ok(text) => text,
                Err(error) => return error,
            }
        }
    };
    shortcut_turn
        .history
        .lock()
        .await
        .push(coco_messages::create_meta_message(&text));

    HandlerResult::ok(coco_types::TurnStartResult {
        turn_id: shortcut_turn.turn_id,
    })
}

async fn handle_turn_start_rename_shortcut(
    rename: coco_commands::ParsedRename,
    ctx: &HandlerContext,
) -> HandlerResult {
    let turn_id = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn.turn_id,
        Err(error) => return error,
    };

    if coco_coordinator::identity::is_teammate() {
        warn!("SDK rename ignored: session is a swarm teammate");
        return HandlerResult::ok(coco_types::TurnStartResult { turn_id });
    }

    let name = match rename {
        coco_commands::ParsedRename::Explicit(name) => name,
        coco_commands::ParsedRename::Auto => {
            let runtime = ctx.resolve_runtime().await;
            let Some(runtime) = runtime else {
                warn!("SDK rename auto-gen ignored: no active session runtime");
                return HandlerResult::ok(coco_types::TurnStartResult { turn_id });
            };
            match crate::session_rename::auto_generate_session_name(&runtime).await {
                Ok(name) => name,
                Err(error) => {
                    warn!(reason = ?error, "SDK rename auto-gen failed");
                    return HandlerResult::ok(coco_types::TurnStartResult { turn_id });
                }
            }
        }
    };

    let Some(session_id) = ctx.target_session_id.clone() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session rename requires an explicitly targeted session".to_string(),
            data: None,
        };
    };
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
            warn!(error = %message, "SDK rename persist failed");
        }
        HandlerResult::NotImplemented(method) => {
            warn!(method = %method, "SDK rename persist failed: handler not implemented");
        }
    }

    HandlerResult::ok(coco_types::TurnStartResult { turn_id })
}

async fn handle_turn_start_memory_shortcut(
    shortcut: TurnStartMemoryShortcut,
    ctx: &HandlerContext,
) -> HandlerResult {
    let shortcut_turn = match mint_shortcut_turn(ctx).await {
        Ok(shortcut_turn) => shortcut_turn,
        Err(error) => return error,
    };

    let runtime = ctx.resolve_runtime().await;
    let Some(runtime) = runtime else {
        match shortcut {
            TurnStartMemoryShortcut::Dream => {
                info!("SDK /dream: no MemoryRuntime (Feature::AutoMemory off); skipping");
            }
            TurnStartMemoryShortcut::Summary => {
                info!("SDK /summary: no MemoryRuntime; skipping");
            }
        }
        emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx).await;
        return HandlerResult::ok(coco_types::TurnStartResult {
            turn_id: shortcut_turn.turn_id,
        });
    };
    let Some(memory_runtime) = runtime.memory_runtime().cloned() else {
        match shortcut {
            TurnStartMemoryShortcut::Dream => {
                info!("SDK /dream: no MemoryRuntime (Feature::AutoMemory off); skipping");
            }
            TurnStartMemoryShortcut::Summary => {
                info!("SDK /summary: no MemoryRuntime; skipping");
            }
        }
        emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx).await;
        return HandlerResult::ok(coco_types::TurnStartResult {
            turn_id: shortcut_turn.turn_id,
        });
    };

    match shortcut {
        TurnStartMemoryShortcut::Dream => {
            let transcript_dir = memory_runtime
                .transcript_dir()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let now_ms = coco_memory::service::dream::DreamService::now_ms();
            let _ = memory_runtime
                .dream
                .force(&transcript_dir, Vec::new, now_ms)
                .await;
        }
        TurnStartMemoryShortcut::Summary => {
            let history = shortcut_turn.history.lock().await.snapshot();
            let tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
            let last_msg_id = history
                .last()
                .and_then(|message| message.uuid())
                .map(uuid::Uuid::to_string);
            let had_tool_calls =
                coco_messages::count_tool_calls_in_last_assistant_turn(history.as_slice()) > 0;
            let _ = memory_runtime
                .session_memory
                .force(tokens, last_msg_id, had_tool_calls)
                .await;
        }
    }

    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx).await;
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

    let response_text = match ctx.resolve_runtime().await {
        None => "(fork dispatcher not installed — /btw requires CLI bootstrap)".to_string(),
        Some(runtime) => {
            let cache = match runtime.last_cache_safe_params().await {
                Some(cache) => cache,
                None => runtime.fallback_cache_safe_params().await,
            };
            match runtime.current_fork_dispatcher().await {
                None => "(fork dispatcher not installed — /btw requires CLI bootstrap)".to_string(),
                Some(dispatcher) => {
                    crate::side_question::run_side_question_fork(
                        &cache,
                        &dispatcher,
                        &request.question,
                    )
                    .await
                }
            }
        }
    };

    let messages = coco_messages::build_slash_command_messages(
        "btw",
        &request.question,
        &response_text,
        /*is_sensitive*/ false,
    );
    {
        let mut history = shortcut_turn.history.lock().await;
        for message in messages {
            let message = Arc::new(message);
            history.push_arc(message.clone());
            let _ = send_session_event(
                &ctx.notif_tx,
                shortcut_turn.session_id.clone(),
                CoreEvent::Protocol(coco_types::ServerNotification::MessageAppended {
                    message,
                    identity: coco_types::ServerNotificationIdentity::default(),
                }),
            )
            .await;
        }
    }
    emit_shortcut_turn_lifecycle(&shortcut_turn, &ctx.notif_tx).await;

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
    let history_handle = Arc::clone(runtime_handle.history());
    let app_state = Arc::clone(runtime_handle.app_state());

    let runtime = &runtime_handle;
    let current_engine_config = runtime.current_engine_config().await;
    let args = crate::goal_command::goal_display_args(&request).to_string();
    let gate = crate::goal_command::GoalGate {
        hooks_restricted: current_engine_config.disable_all_hooks
            || current_engine_config.allow_managed_hooks_only,
        // SDK is non-interactive; the trust gate is deliberately skipped.
        trust_rejected: false,
    };
    let tokens_at_start = runtime.session_usage_snapshot().await.totals.output_tokens;
    let history_snapshot = history_handle.lock().await.to_vec();
    let outcome = crate::goal_command::resolve_goal_request(
        request,
        &app_state,
        &runtime.hook_registry(),
        &history_snapshot,
        tokens_at_start,
        gate,
    )
    .await;
    let session_id = runtime_handle.session_id().clone();

    match outcome {
        crate::goal_command::GoalOutcome::Text(text) => {
            let shortcut_turn = mint_shortcut_turn(ctx).await?;
            sdk_append_slash_text(
                &history_handle,
                &ctx.notif_tx,
                session_id,
                "goal",
                &args,
                &text,
            )
            .await;
            Ok(TurnStartGoalShortcut::Complete(HandlerResult::ok(
                coco_types::TurnStartResult {
                    turn_id: shortcut_turn.turn_id,
                },
            )))
        }
        crate::goal_command::GoalOutcome::StatusThenText { status, text } => {
            let shortcut_turn = mint_shortcut_turn(ctx).await?;
            sdk_append_goal_status_and_slash_text(
                &runtime_handle,
                &history_handle,
                &ctx.notif_tx,
                status,
                &args,
                &text,
            )
            .await;
            sdk_emit_active_goal_snapshot(&runtime_handle, &app_state, &ctx.notif_tx).await;
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
            sdk_append_goal_status(&history_handle, &ctx.notif_tx, session_id.clone(), status)
                .await;
            sdk_emit_active_goal_snapshot(&runtime_handle, &app_state, &ctx.notif_tx).await;
            sdk_append_slash_text(
                &history_handle,
                &ctx.notif_tx,
                session_id,
                "goal",
                &args,
                &text,
            )
            .await;
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
        return active_turn_start_error(ActiveTurnStartError::NoActiveSession);
    };
    let turn_id = match start_active_turn_for_runtime(
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
                // The active-turn record is cleared by the forwarder as it
                // forwards the terminal `TurnEnded`, not here.
                run_manual_compact_shortcut(
                    runtime_for_task,
                    request,
                    turn_id_for_task.clone(),
                    inner_tx,
                    cancel_token_for_task,
                )
                .await;
            });

            info!(turn_id = %turn_id_for_log, "SdkServer: /compact shortcut");
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

fn start_active_turn_for_runtime(
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

async fn run_manual_compact_shortcut(
    session: crate::session_runtime::SessionHandle,
    request: coco_commands::handlers::compact::CompactRequest,
    turn_id: TurnId,
    event_tx: mpsc::Sender<CoreEvent>,
    cancel: CancellationToken,
) {
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::TurnStarted(coco_types::TurnStartedParams {
                turn_id: turn_id.clone(),
            }),
        ))
        .await;
    let runtime = &session;
    let engine = runtime.build_engine(cancel).await;
    let mut history = runtime.history().lock().await.snapshot();

    let command_args = request.custom_instructions;
    let custom_instructions = if command_args.is_empty() {
        None
    } else {
        Some(command_args.clone())
    };
    let event_tx_opt = Some(event_tx.clone());
    let request = coco_query::ManualCompactRequest {
        custom_instructions,
        command_args,
    };
    engine
        .run_manual_compact(&mut history, &event_tx_opt, request)
        .await;

    {
        let mut runtime_history = runtime.history().lock().await;
        *runtime_history = history;
    }

    let session_id = runtime.current_typed_session_id().await.to_string();
    let manager = Arc::clone(runtime.session_manager());
    let _ =
        tokio::task::spawn_blocking(move || manager.re_append_session_metadata(&session_id)).await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::TurnEnded(coco_types::TurnEndedParams::completed(
                turn_id,
                Some(coco_types::TokenUsage::default()),
                Some(coco_messages::StopReason::EndTurn),
            )),
        ))
        .await;
}

async fn sdk_append_slash_text(
    history_handle: &Arc<tokio::sync::Mutex<coco_messages::MessageHistory>>,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    session_id: coco_types::SessionId,
    command: &str,
    args: &str,
    text: &str,
) {
    let messages = coco_messages::build_slash_command_messages(
        command, args, text, /*is_sensitive*/ false,
    );
    sdk_append_messages(history_handle, notif_tx, session_id, messages).await;
}

async fn sdk_append_goal_status(
    history_handle: &Arc<tokio::sync::Mutex<coco_messages::MessageHistory>>,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    session_id: coco_types::SessionId,
    payload: coco_types::GoalStatusPayload,
) {
    sdk_append_messages(
        history_handle,
        notif_tx,
        session_id,
        vec![coco_messages::Message::Attachment(
            coco_messages::AttachmentMessage::silent_goal_status(payload),
        )],
    )
    .await;
}

async fn sdk_append_goal_status_and_slash_text(
    session: &crate::session_runtime::SessionHandle,
    history_handle: &Arc<tokio::sync::Mutex<coco_messages::MessageHistory>>,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let mut messages = vec![coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(payload),
    )];
    messages.extend(coco_messages::build_slash_command_messages(
        "goal", args, text, /*is_sensitive*/ false,
    ));
    sdk_append_messages(
        history_handle,
        notif_tx,
        session.session_id().clone(),
        messages.clone(),
    )
    .await;
    session.persist_local_transcript_messages(&messages).await;
}

async fn sdk_emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    app_state: &tokio::sync::RwLock<coco_types::ToolAppState>,
    notif_tx: &mpsc::Sender<OutboundMessage>,
) {
    let goal = app_state.read().await.active_goal.clone();
    let _ = send_session_event(
        notif_tx,
        session.session_id().clone(),
        CoreEvent::Protocol(crate::goal_command::active_goal_changed_notification(
            goal.clone(),
        )),
    )
    .await;
    session
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

async fn sdk_append_messages(
    history_handle: &Arc<tokio::sync::Mutex<coco_messages::MessageHistory>>,
    notif_tx: &mpsc::Sender<OutboundMessage>,
    session_id: coco_types::SessionId,
    messages: Vec<coco_messages::Message>,
) {
    let mut history = history_handle.lock().await;
    for message in messages {
        let message = Arc::new(message);
        history.push_arc(message.clone());
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
) {
    let turn_id = shortcut_turn.turn_id.clone();
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
                Some(coco_messages::StopReason::EndTurn),
            ),
        )),
    )
    .await;
}

async fn mint_shortcut_turn(ctx: &HandlerContext) -> Result<ShortcutTurnState, HandlerResult> {
    let runtime = ctx
        .resolve_runtime()
        .await
        .ok_or(ActiveTurnStartError::NoActiveSession)
        .map_err(active_turn_start_error)?;
    if runtime.has_active_turn() {
        return Err(active_turn_start_error(
            ActiveTurnStartError::TurnAlreadyRunning,
        ));
    }
    let session_id = runtime.session_id().clone();
    let turn_id = runtime.next_turn_id();
    Ok(ShortcutTurnState {
        session_id,
        turn_id,
        history: Arc::clone(runtime.history()),
    })
}

fn active_turn_start_error(error: ActiveTurnStartError) -> HandlerResult {
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

fn decode_session_cost_text(result: HandlerResult) -> Result<String, HandlerResult> {
    match result {
        HandlerResult::Ok(value) => serde_json::from_value::<coco_types::SessionCostResult>(value)
            .map(|result| result.text)
            .map_err(|error| HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("session/cost result decode failed: {error}"),
                data: None,
            }),
        error @ HandlerResult::Err { .. } | error @ HandlerResult::NotImplemented(_) => Err(error),
    }
}

fn decode_session_status_text(result: HandlerResult) -> Result<String, HandlerResult> {
    match result {
        HandlerResult::Ok(value) => {
            serde_json::from_value::<coco_types::SessionStatusResult>(value)
                .map(|result| result.text)
                .map_err(|error| HandlerResult::Err {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("session/status result decode failed: {error}"),
                    data: None,
                })
        }
        error @ HandlerResult::Err { .. } | error @ HandlerResult::NotImplemented(_) => Err(error),
    }
}

/// `turn/interrupt` — cancel the currently-running turn (if any).
///
/// Cancellation is cooperative: the runner's task is notified via the
/// `CancellationToken` it received from `turn/start`. The runner is
/// expected to observe `cancel.is_cancelled()` at tool boundaries and
/// emit a `turn/failed` notification before exiting.
pub(super) async fn handle_turn_interrupt(ctx: &HandlerContext) -> HandlerResult {
    let Some(session) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session".into(),
            data: None,
        };
    };
    let session_id = session.session_id().clone();
    match session.active_turn_cancel_token() {
        Some(token) => {
            info!(session_id = %session_id, "SdkServer: turn/interrupt");
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
/// The dispatcher holds pending approvals keyed by `request_id`. When the agent's
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
    match resolve_app_server_request(
        ctx,
        coco_app_server::ServerRequestReply::Approval(params),
        "approval",
    ) {
        Ok(()) => {
            info!(request_id = %request_id, decision = ?decision, "SdkServer: approval/resolve");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
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
    match resolve_app_server_request(
        ctx,
        coco_app_server::ServerRequestReply::Elicitation(params),
        "elicitation",
    ) {
        Ok(()) => {
            info!(request_id = %request_id, mcp_server = %mcp_server, approved, "SdkServer: elicitation/resolve");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
}

/// `input/resolveUserInput` — resolve a pending `input/requestUserInput`
/// ServerRequest with the user's answer (free-form or multiple-choice).
pub(super) async fn handle_user_input_resolve(
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
            info!(request_id = %request_id, "SdkServer: input/resolveUserInput");
            HandlerResult::ok_empty()
        }
        Err(error) => error,
    }
}

/// `control/cancelRequest` — cancel a previously-issued ServerRequest.
///
/// The SDK client uses this to abort a `ServerRequest::AskForApproval`
/// (or similar) that it no longer wants to resolve, e.g. if the user
/// closed the approval UI before answering.
///
/// We drop the pending oneshot sender so the agent-side receiver gets
/// an `Err (RecvError)` and the tool executor can treat it as "denied".
/// If the `request_id` isn't in any pending map, we still return ok so
/// the client doesn't treat a race (server already resolved + cleaned
/// up) as a protocol error.
pub(super) async fn handle_cancel_request(
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
            let error = crate::sdk_server::session_lifecycle::app_server_lifecycle_error(
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
