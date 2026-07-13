//! Production [`TurnRunner`](super::TurnRunner) backed by [`coco_query::QueryEngine`].
//!
//! This is the bridge between AppServer request dispatch and the real agent
//! loop. The CLI entry point in `main.rs` constructs one of these per-process
//! and installs it on the host state.
//!
//! Scope:
//! - One QueryEngine per turn (fresh config). Multi-turn context is
//!   read and updated through the selected `SessionHandle`: the runner
//!   appends turn input, calls `run_with_messages`, and replaces the
//!   history with `result.final_messages` on completion.
//! - Forwards CoreEvents emitted by the engine onto the host outbound queue.
//!   Local and remote adapters then map those events to their own surfaces.
//!
//! AppServer clients drive the cadence via multiple `turn/start` calls
//! per session.

use std::{pin::Pin, sync::Arc};

use coco_types::{CoreEvent, TurnStartParams};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::TurnRunner;

/// Process-stateless executor. Every call receives the AppServer-selected
/// session capability explicitly.
pub struct SessionTurnExecutor {
    max_turns: Option<i32>,
    system_prompt: Option<String>,
}

impl SessionTurnExecutor {
    pub fn new(max_turns: Option<i32>, system_prompt: Option<String>) -> Self {
        Self {
            max_turns,
            system_prompt,
        }
    }
}

impl TurnRunner for SessionTurnExecutor {
    fn run_turn<'a>(
        &'a self,
        session: crate::session_runtime::SessionHandle,
        app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        params: TurnStartParams,
        turn_id: coco_types::TurnId,
        event_tx: mpsc::Sender<CoreEvent>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let max_turns = self.max_turns;
        let system_prompt = self.system_prompt.clone();
        Box::pin(async move {
            run_turn_with_session(
                session,
                app_server,
                max_turns,
                system_prompt,
                params,
                turn_id,
                event_tx,
                cancel,
            )
            .await
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn_with_session(
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    max_turns: Option<i32>,
    system_prompt: Option<String>,
    params: TurnStartParams,
    turn_id: coco_types::TurnId,
    event_tx: mpsc::Sender<CoreEvent>,
    cancel: CancellationToken,
) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>> {
    let prompt = params.prompt;
    let images = params.images;
    let history_override = params.history_override;
    let slash_metadata = params.slash_metadata.clone();
    let model_selection_override = params.model_selection.clone();
    let permission_mode_override = params.permission_mode;
    let thinking_level_override = params.thinking_level;
    let session_id = session.session_id().clone();
    // Keep our own handle on the cancel token. The engine consumes
    // its copy; we still need to know post-run whether the user
    // requested an interrupt so the wire stream gets `turn/interrupted`
    // rather than `turn/failed`.
    let cancel_for_terminal = cancel.clone();
    Box::pin(async move {
        let turn_engine = session
            .build_turn_engine(
                crate::session_runtime::SessionTurnEngineConfigRequest {
                    model_selection: model_selection_override,
                    permission_mode: permission_mode_override,
                    thinking_level: thinking_level_override,
                    max_turns,
                    system_prompt,
                },
                cancel,
            )
            .await;
        let turn_cwd = turn_engine.turn_cwd.clone();
        info!(
            session_id = %session_id,
            model = %turn_engine.model_id,
            cwd = %turn_cwd.display(),
            "SessionTurnExecutor: run_turn"
        );

        let engine = turn_engine.engine.with_permission_bridge(Arc::new(
            super::AppServerPermissionBridge::new(Arc::clone(&app_server), session.clone()),
        ));

        // Snapshot the prior history, append a fresh user message,
        // and **persist the combined history back to shared state
        // BEFORE calling the engine**. This way, even if the engine
        // returns `Err(...)` (e.g. transport crash, unrecoverable
        // tool failure), the user's prompt is still recorded and
        // the next `turn/start` sees it. On `Ok`, we overwrite with
        // the engine's more up-to-date `final_messages`, which also
        // includes any tool calls + the assistant reply.
        //
        // The engine's `run_session_loop` finds the LAST user
        // message in the list and keys the file history snapshot
        // against it, so passing the whole combined list works
        // for both single and multi-turn scenarios.
        // The handler minted and returned this id in the synchronous
        // `turn/start` response; lifecycle events must use the same id so
        // clients can correlate completion.
        let cycle_turn_id = turn_id;

        let combined: Vec<std::sync::Arc<coco_messages::Message>> = if history_override.is_empty() {
            // Fire UserPromptSubmit hooks BEFORE the LLM call. Output
            // surfaces as `hook_*` reminders on the next reminder pass;
            // a blocking_error suppresses the turn (warns instead);
            // prevent_continuation keeps the prompt but skips the
            // engine.
            let prompt_hook_result = session.fire_user_prompt_submit_hooks(&prompt).await;
            if let Some(blocking) = &prompt_hook_result.blocking_error {
                let warning = format!(
                    "UserPromptSubmit hook blocked the turn: {}\n\nOriginal prompt: {prompt}",
                    blocking.blocking_error,
                );
                let warning_msg = std::sync::Arc::new(coco_messages::create_user_message(&warning));
                // I-1: emit so AppServer observers see the warning row.
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::MessageAppended {
                            message: warning_msg,
                            identity: coco_types::ServerNotificationIdentity::default(),
                        },
                    ))
                    .await;
                // Pre-engine bail: emit a self-contained
                // TurnStarted + TurnEnded(Failed) pair so AppServer
                // consumers see a complete cycle envelope. `HookBlocked`
                // is the typed signal that this is a policy decision,
                // not a runtime/config/provider error — lets dashboards
                // filter "real failures" from "hook said no".
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::TurnStarted(
                            coco_types::TurnStartedParams {
                                turn_id: cycle_turn_id.clone(),
                            },
                        ),
                    ))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::TurnEnded(
                            coco_types::TurnEndedParams::failed(
                                cycle_turn_id.clone(),
                                /*usage*/ None,
                                coco_types::ErrorPayload {
                                    message: warning.clone(),
                                    code: coco_types::ErrorCode::HookBlocked,
                                },
                            ),
                        ),
                    ))
                    .await;
                return Ok(());
            }
            if prompt_hook_result.prevent_continuation {
                let stop_msg = prompt_hook_result
                    .stop_reason
                    .clone()
                    .map(|r| format!("Operation stopped by hook: {r}"))
                    .unwrap_or_else(|| "Operation stopped by hook".to_string());
                let prompt_msg = std::sync::Arc::new(coco_messages::create_user_message(&prompt));
                let stop_msg_obj =
                    std::sync::Arc::new(coco_messages::create_user_message(&stop_msg));
                // I-1: emit so AppServer observers see both rows.
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::MessageAppended {
                            message: prompt_msg,
                            identity: coco_types::ServerNotificationIdentity::default(),
                        },
                    ))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::MessageAppended {
                            message: stop_msg_obj,
                            identity: coco_types::ServerNotificationIdentity::default(),
                        },
                    ))
                    .await;
                return Ok(());
            }

            // Resolve `@`-mentions in the prompt to file-content
            // system-reminder messages. A shared helper drives TUI /
            // headless / AppServer clients identically — without this, headless and
            // AppServer clients sending `@path/to/file` got the literal string
            // instead of the file's contents (the `at_mentioned_files`
            // reminder body claims content is "loaded into context" —
            // this is what makes that true).
            let inputs = crate::at_mention_turn::resolve_turn_inputs(
                &prompt,
                &images,
                &turn_cwd,
                uuid::Uuid::new_v4(),
                session.file_read_state(),
            )
            .await;
            let mut new_msgs = Vec::new();
            if let Some(metadata) = slash_metadata.as_deref() {
                new_msgs.push(crate::session_messages::slash_metadata_message(metadata));
            }
            new_msgs.extend(crate::at_mention_turn::build_messages_for_turn(&inputs));
            // I-1 (Authority) — D2: emit MessageAppended for the new
            // turn messages BEFORE invoking the engine. The engine no
            // longer re-emits its initial turn_messages load (would
            // double-fire on every turn). Engines only emit for
            // newly-produced content (assistant turns, tool results,
            // system pushes) within the loop. See
            // `engine-tui-unified-transcript-plan.md` §5.2.
            for m in new_msgs.iter().cloned() {
                let _ = event_tx
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::MessageAppended {
                            message: std::sync::Arc::new(m),
                            identity: coco_types::ServerNotificationIdentity::default(),
                        },
                    ))
                    .await;
            }
            let new_msg_arcs: Vec<std::sync::Arc<coco_messages::Message>> =
                new_msgs.into_iter().map(std::sync::Arc::new).collect();
            let combined = session
                .append_arc_messages_to_history_and_snapshot(new_msg_arcs)
                .await;
            if !inputs.mentioned_paths.is_empty() {
                engine
                    .note_mentioned_paths(inputs.mentioned_paths.clone())
                    .await;
            }
            combined
        } else {
            let override_messages: Vec<std::sync::Arc<coco_messages::Message>> = history_override
                .into_iter()
                .map(serde_json::from_value::<coco_messages::Message>)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(std::sync::Arc::new)
                .collect();
            session
                .replace_history_with_arc_messages(override_messages.clone())
                .await;
            override_messages
        };

        // Clone the event channel so we can still emit on the
        // error path (the engine takes ownership of the original).
        let event_tx_for_error = event_tx.clone();
        let session_id_for_error = session_id.clone();
        let (core_event_tx, mut core_event_rx) = mpsc::channel::<CoreEvent>(256);
        let event_tx_forward = event_tx.clone();
        let session_for_forward = session.clone();
        let forward_handle = tokio::spawn(async move {
            // Hold the terminal `TurnEnded` so the lifecycle owner (this turn
            // task) can commit final history/accounting BEFORE the forwarder
            // clears the turn slot and delivers the terminal. Otherwise a next
            // `turn/start` admitted the instant the client sees `TurnEnded`
            // could be built against stale history (CS-2 / R13). Every other
            // event still forwards inline.
            let mut pending_terminal: Option<CoreEvent> = None;
            while let Some(event) = core_event_rx.recv().await {
                if matches!(
                    event,
                    CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(_))
                ) {
                    pending_terminal = Some(event);
                    continue;
                }
                if matches!(
                    event,
                    CoreEvent::Protocol(coco_types::ServerNotification::ContextCompacted(_))
                ) {
                    session_for_forward.re_append_session_metadata().await;
                }
                if event_tx_forward.send(event).await.is_err() {
                    return None;
                }
            }
            pending_terminal
        });

        let engine_result = engine
            .run_with_messages(combined, core_event_tx, cycle_turn_id.clone())
            .await;
        let pending_terminal = forward_handle.await.ok().flatten();

        match engine_result {
            Ok(result) => {
                info!(
                    turns = result.turns,
                    input_tokens = result.total_usage.input_tokens.total,
                    output_tokens = result.total_usage.output_tokens.total,
                    history_len = result.final_messages.len(),
                    "SessionTurnExecutor: turn complete"
                );
                // Overwrite with the engine's final history — this
                // includes tool calls, tool results, and the
                // assistant reply in addition to the user message
                // we pre-persisted above.
                let final_history = result.final_history.snapshot();
                session.commit_engine_turn_history(final_history).await;
                // Sole Interrupted emit site. Fires when either the
                // engine observed cancel mid-loop (`result.cancelled`
                // = true → engine returned Ok with cancelled marker)
                // OR the cancel raced and arrived after Ok return
                // (`cancel_for_terminal.is_cancelled()`). The engine
                // no longer wire-emits Interrupted — runner owns the
                // single terminator. Reason is hardcoded
                // `UserCancel`: AppServer turn cancellation reaches this
                // runner via `turn/interrupt`, which is by definition
                // user-initiated. (TUI has the broader
                // UserCancel-vs-SystemPreempt split because of local control
                // arms like `/clear` / `/compact` / `/rewind`. AppServer
                // control shortcuts are intercepted before this runner, so
                // normal runner cancellation is user interrupt only.)
                if result.cancelled || cancel_for_terminal.is_cancelled() {
                    let reason = match result.stop_reason.as_deref() {
                        Some("permission_abort") => coco_types::TurnAbortReason::PermissionAbort,
                        _ => coco_types::TurnAbortReason::UserCancel,
                    };
                    let _ = event_tx_for_error
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::TurnEnded(
                                coco_types::TurnEndedParams::interrupted(
                                    cycle_turn_id.clone(),
                                    /*usage*/ None,
                                    reason,
                                ),
                            ),
                        ))
                        .await;
                } else if let Some(terminal) = pending_terminal {
                    // Deliver the held terminal only AFTER history/accounting
                    // are committed, so a next turn admitted on terminal receipt
                    // observes the completed history (CS-2 / R13). A cancelled
                    // turn supersedes it with the Interrupted terminator above.
                    let _ = event_tx_for_error.send(terminal).await;
                }
                Ok(())
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "SessionTurnExecutor: engine returned error; \
                     user message already persisted to session history"
                );
                // Engine-bail path: when cancel was the cause the
                // engine_session Err branch skipped its `Failed`
                // emit, so we synthesize the Interrupted terminator
                // here. When it's a true error the engine_session
                // already emitted `Failed` — no second terminator
                // needed.
                if cancel_for_terminal.is_cancelled() {
                    let _ = event_tx_for_error
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::TurnEnded(
                                coco_types::TurnEndedParams::interrupted(
                                    cycle_turn_id.clone(),
                                    /*usage*/ None,
                                    coco_types::TurnAbortReason::UserCancel,
                                ),
                            ),
                        ))
                        .await;
                }

                // Emit a synthetic `SessionResult` with `is_error=true`
                // so the forwarder's `accumulate_session_result` folds
                // the failure into the AppServer session stats accumulator. Without
                // this, true engine-bail paths (compaction failure,
                // transport crash, etc.) don't surface in the final
                // aggregated `SessionResult` emitted by `session/close`.
                //
                // Fields are minimal — we don't have usage/cost
                // because the engine didn't reach `make_result`. The
                // forwarder handles missing fields gracefully (default
                // usage is zero; cost is 0.0; errors list is the one
                // message we provide).
                let error_params = coco_types::SessionResultParams {
                    session_id: session_id_for_error.clone(),
                    total_turns: 1,
                    duration_ms: 0,
                    duration_api_ms: 0,
                    is_error: true,
                    stop_reason: if cancel_for_terminal.is_cancelled() {
                        "interrupted".into()
                    } else {
                        "engine_error".into()
                    },
                    total_cost_usd: 0.0,
                    usage: coco_types::TokenUsage::default(),
                    model_usage: std::collections::HashMap::new(),
                    permission_denials: Vec::new(),
                    result: None,
                    errors: vec![e.to_string()],
                    structured_output: None,
                    fast_mode_state: None,
                    num_api_calls: None,
                };
                let _ = event_tx_for_error
                    .send(CoreEvent::Protocol(
                        coco_types::ServerNotification::SessionResult(Box::new(error_params)),
                    ))
                    .await;
                Err(anyhow::anyhow!("{e}"))
            }
        }
    })
}

#[cfg(test)]
#[path = "session_turn_executor.test.rs"]
mod tests;
