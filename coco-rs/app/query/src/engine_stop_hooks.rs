//! Stop-hook dispatcher for the no-tool-calls terminal path.
//!
//! Dispatches the `handleStopHooks` flow and adds the
//! `isApiErrorMessage` short-circuit that prevents a death spiral
//! when the last assistant message carries an `api_error` value —
//! Finding **C3**.
//!
//! Uses [`coco_types::ApiError`] on
//! [`coco_messages::AssistantMessage::api_error`] as the typed
//! predicate — no string-matching, multi-provider-safe.
//!
//! ## Out of scope here
//!
//! - StructuredOutput retry-cap terminal — stays inline in `engine.rs`
//!   ahead of the dispatcher call.
//! - `flush_successful_turn_state` / `maybe_spawn_prompt_suggestion_after_stop`
//!   — transcript / fork-spawn side effects that fire on both the
//!   block-pre and the clean-end paths. Caller controls ordering.
//! - Token-budget continuation — orthogonal "should we squeeze in
//!   one more turn?" check that runs only when stop hooks pass
//!   cleanly. Caller-driven.

use coco_hooks::orchestration;
use coco_messages::Message;
use coco_messages::MessageHistory;
use tracing::info;
use tracing::warn;

use crate::config::ContinueReason;
use crate::engine::QueryEngine;
use crate::engine_loop_state::LoopTurnState;

#[cfg(test)]
#[path = "engine_stop_hooks.test.rs"]
mod tests;

/// What [`QueryEngine::run_stop_hooks`] decided. The four variants
/// Map to the four exit shapes of the stop hook arm.
#[derive(Debug)]
pub(crate) enum StopHookDecision {
    /// `isApiErrorMessage(lastMessage)` returned `true`: skip stop hooks AND
    /// token-budget continuation; let the natural end-turn path close out so the
    /// user sees the api_error explanation and a fresh turn can be
    /// initiated by user input. Finding **C3**.
    ///
    /// `error_type` carries the short canonical code lifted off the
    /// trailing assistant's [`coco_types::ApiError::error_type`]
    /// (`prompt_too_long` / `max_output_tokens` / `content_filter` /
    /// `invalid_request` / `model_error` / …). The engine uses it as
    /// the `QueryResult.stop_reason` so SDK consumers see the typed
    /// reason code (Finding **R1** — without this lift, every
    /// SkippedApiError exit collapsed to the generic
    /// `"end_turn_api_error"`). `None` only when the synthesis site
    /// didn't classify; the caller falls back to that legacy label.
    SkippedApiError { error_type: Option<String> },
    /// No hooks installed / all hooks passed cleanly. Caller proceeds
    /// to the token-budget continuation check, then the clean
    /// end-turn emit.
    Continue,
    /// A Stop hook returned `block` with a `blocking_error` feedback
    /// message. The dispatcher already pushed the feedback meta
    /// message to `history` and called `flush_successful_turn_state`
    /// to persist the transcript through the blocking attempt;
    /// caller writes `turn_state.transition = Some(StopHookBlocking)`
    /// (already done internally) and `continue`s the outer loop.
    BlockedContinueLoop,
    /// A Stop hook returned `prevent_continuation`. Caller should
    /// return `QueryResult { stop_reason: "stop_hook_prevented" }`
    /// after running any post-turn flush helpers it owns.
    Prevented,
}

/// `ApiError` payload lifted off the most recent assistant message,
/// when present. Returned by [`last_assistant_api_error_payload`] and
/// consumed by the C3 death-spiral guard to populate the StopFailure
/// hook input.
#[derive(Debug, Clone)]
pub(crate) struct LastApiErrorPayload {
    /// Human-readable details.
    pub(crate) message: String,
    /// Short canonical code. `None` when the synthesis site didn't
    /// classify the error — the C3 guard then falls back to `"unknown"`.
    pub(crate) error_type: Option<String>,
}

/// Extract the `ApiError` payload from the most recent assistant
/// message in `history`, when present. `Some(_)` is the typed
/// predicate that drives the C3 death-spiral short-circuit; the payload
/// is forwarded to `executeStopFailureHooks` so hook matchers can
/// filter by specific error code. Walks backwards; ignores tool
/// results / attachments / system messages / progress / tombstones /
/// user trailers.
fn last_assistant_api_error_payload(history: &MessageHistory) -> Option<LastApiErrorPayload> {
    history
        .as_slice()
        .iter()
        .rev()
        .find_map(|m| match m.as_ref() {
            Message::Assistant(a) => Some(a.api_error.as_ref().map(|e| LastApiErrorPayload {
                message: e.message.clone(),
                error_type: e.error_type.clone(),
            })),
            // Tool results / attachments / system messages / progress /
            // tombstones / user trailers don't count — keep walking
            // until we find the most recent assistant message.
            Message::User(_)
            | Message::System(_)
            | Message::Attachment(_)
            | Message::ToolResult(_)
            | Message::Progress(_)
            | Message::Tombstone(_) => None,
        })
        .flatten()
}

impl QueryEngine {
    /// Drive the Stop hook pipeline for the no-tool-calls terminal.
    ///
    /// Order of operations:
    ///
    /// 1. Check `last_assistant_api_error_message(history)` — if
    ///    `Some(_)`, fire StopFailure hooks then return
    ///    [`StopHookDecision::SkippedApiError`] (Finding C3).
    /// 2. If `self.hooks` is `None`, return [`StopHookDecision::Continue`].
    /// 3. Invoke `coco_hooks::orchestration::execute_stop` with the
    ///    current `stop_hook_active` flag and the assistant text
    ///    extracted from `response_text` (None when empty — stop hooks
    ///    receive optional last text).
    /// 4. Map the [`coco_hooks::orchestration::AggregatedHookResult`]:
    ///    - `prevent_continuation` ⇒ [`StopHookDecision::Prevented`].
    ///    - `blocking_error` ⇒ push feedback meta message via
    ///      `history_sync::history_push_and_emit`, run
    ///      `flush_successful_turn_state`, set
    ///      `turn_state.transition = StopHookBlocking` and
    ///      `turn_state.stop_hook_active = true`, return
    ///      [`StopHookDecision::BlockedContinueLoop`].
    ///    - clean pass ⇒ [`StopHookDecision::Continue`].
    ///
    /// The dispatcher handles the transcript persistence (`flush_successful_turn_state`)
    /// on the blocking path so the same call appears in both the
    /// prevent-block and the block-and-retry exit shapes without the
    /// caller having to duplicate the flush. The caller still owns
    /// the pre-dispatcher `flush + maybe_spawn_prompt_suggestion`
    /// pair because those fire for every Stop entry regardless
    /// of hook outcome.
    pub(crate) async fn run_stop_hooks(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        hook_tx_opt: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        turn_state: &mut LoopTurnState,
        response_text: &str,
    ) -> StopHookDecision {
        // Finding C3: death-spiral guard. Skip Stop hooks entirely
        // when the last assistant message is an api_error —
        // the model never produced a real response, so hooks
        // evaluating it would just block, the engine would retry, the
        // retry would re-emit the api_error, the hooks would block
        // again, ad infinitum.
        if let Some(payload) = last_assistant_api_error_payload(history) {
            info!(
                error_type = payload.error_type.as_deref().unwrap_or("unknown"),
                "skipping Stop hooks — last assistant message is api_error \
                 (C3 death-spiral guard)"
            );
            // Fire StopFailure hooks before returning from the api_error
            // short-circuit so observability / cleanup handlers still see
            // the terminal signal. Fire-and-forget; swallow registry errors
            // per the established
            // engine_session.rs:246-256 pattern.
            //
            // `error_label` is the canonical short code
            // (`max_output_tokens` / `prompt_too_long` /
            // `content_filter` / `model_error` / …) so hook matchers
            // can filter by specific error.
            if let Some(hooks) = &self.hooks {
                let hook_ctx = self.orchestration_ctx();
                let error_label = payload.error_type.as_deref().unwrap_or("unknown");
                if let Err(e) = orchestration::execute_stop_failure(
                    hooks,
                    &hook_ctx,
                    error_label,
                    Some(payload.message.as_str()),
                    /*last_assistant_message*/ None,
                )
                .await
                {
                    warn!(
                        error = %e,
                        "StopFailure hook execution failed (C3 api_error path)"
                    );
                }
            }
            return StopHookDecision::SkippedApiError {
                error_type: payload.error_type,
            };
        }

        let Some(hooks) = &self.hooks else {
            return StopHookDecision::Continue;
        };

        let hook_ctx = self.orchestration_ctx();
        let last_assistant_message = if response_text.is_empty() {
            None
        } else {
            Some(response_text)
        };
        let history_snapshot = history.to_vec();

        match orchestration::execute_stop(
            hooks,
            &hook_ctx,
            turn_state.stop_hook_active,
            last_assistant_message,
            &history_snapshot,
            hook_tx_opt,
        )
        .await
        {
            Ok(agg) if agg.prevent_continuation => {
                info!("Stop hook prevented continuation");
                StopHookDecision::Prevented
            }
            Ok(agg) if agg.is_blocked() => {
                if let Some(err) = &agg.blocking_error {
                    let feedback = orchestration::format_stop_hook_message(err);
                    warn!(%feedback, "Stop hook blocked session completion");
                    crate::history_sync::history_push_and_emit(
                        history,
                        coco_messages::create_meta_message(&feedback),
                        event_tx,
                    )
                    .await;
                    self.flush_successful_turn_state(history).await;
                    turn_state.transition = Some(ContinueReason::StopHookBlocking);
                    // Mark the recursion so the next Stop firing carries
                    // `stop_hook_active: true`.
                    turn_state.stop_hook_active = true;
                    // **Finding R3** — resets
                    // `maxOutputTokensRecoveryCount: 0` on the
                    // stop-hook-blocking continue. Without this reset,
                    // a turn that used N max_tokens recovery attempts
                    // before being blocked sees only `LIMIT - N`
                    // remaining attempts after the retry, when a fresh
                    // recovery cycle would have a full budget. The
                    // stop-hook-blocking branch is a fresh attempt from
                    // the user-prompt perspective; the counter belongs
                    // to the previous attempt only.
                    turn_state.max_tokens_recovery_count = 0;
                    StopHookDecision::BlockedContinueLoop
                } else {
                    // Defensive: `is_blocked()` returned true but
                    // `blocking_error` is None. Shouldn't happen given the
                    // aggregator's invariants; treat as clean pass.
                    StopHookDecision::Continue
                }
            }
            Ok(_) => StopHookDecision::Continue,
            Err(e) => {
                warn!(error = %e, "Stop hook execution failed");
                StopHookDecision::Continue
            }
        }
    }
}
