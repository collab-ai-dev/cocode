//! Stream-consume layer for the agent loop.
//!
//! ## Owns
//!
//! 1. [`WithheldReason`] + [`withhold_reason_for_stop`] — the typed
//!    enum and the single `StopReason → WithheldReason` mapping the
//!    `engine_recovery` dispatcher matches on. Variants are
//!    provider-agnostic: the Anthropic / OpenAI / Google / ByteDance /
//!    OpenAI-compatible adapters all map their context-overflow /
//!    output-cap signals to the same `coco_inference::StopReason`
//!    (re-exported as `coco_messages::StopReason`).
//!
//! 2. [`StreamOutcome`] + [`StreamConsumed`] — the typed result of
//!    consuming one LLM stream to completion. Replaces the previous
//!    bundle of locals (`response_text`, `reasoning_text`,
//!    `tool_order`, `tool_buffers`, `stream_usage`, `stream_stop_reason`,
//!    `stream_error`, `turn_snapshot`) that floated through
//!    `run_session_loop`. The struct's accumulator fields are always
//!    populated; `outcome` discriminates the three terminal states
//!    (Finish / Error / Cancel) so the caller's post-stream branch can
//!    `match` exhaustively.
//!
//! 3. [`QueryEngine::consume_stream`] — drives the per-turn stream
//!    loop end-to-end and returns the typed outcome. Extracted from
//!    `run_session_loop` so the main loop reads as a sequence of
//!    phase calls (Phase 9 stream consume → post-stream branch)
//!    rather than embedding 360 LoC of event matching inline.
//!
//! The Rust split is structurally different from the JS original
//! (Rust doesn't have JS generators, so the loop is a normal `async fn`),
//! but the semantics map 1:1.

use std::collections::HashMap;
use std::sync::Arc;

use coco_inference::AssistantTurnSnapshot;
use coco_inference::StreamEvent;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_messages::AssistantMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::StopReason;
use coco_tool_runtime::PreparedToolCall;
use coco_tool_runtime::RunOneRuntime;
use coco_tool_runtime::StreamingHandle;
use coco_tool_runtime::ToolCallPlan;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::UnstampedToolCallOutcome;
use coco_types::TokenUsage;
use tracing::warn;

use crate::engine::QueryEngine;
use crate::engine_helpers::StreamingToolCallBuffer;
use crate::engine_loop_state::LoopAccumulator;
use crate::engine_loop_state::LoopConstants;
use crate::engine_loop_state::LoopServices;
use crate::engine_loop_state::LoopTurnState;
use crate::session_state::SessionStateTracker;

/// Why an end-of-stream signal should be kept out of the user-visible
/// event stream pending a recovery attempt. Each variant carries no
/// payload because the recovery dispatcher reads the assistant message
/// + the current loop state for everything else it needs.
///
/// Provider-agnostic — these variants do **not** name Anthropic /
/// OpenAI / Google. The provider seam in `vercel-ai-*` normalizes
/// raw provider signals (`prompt_too_long`, `length`, `content_filter`,
/// `SAFETY`, `RECITATION`, `imageTooLarge`, …) to the typed
/// `coco_inference::StopReason` / `coco_inference::InferenceError`
/// before this layer ever sees them.
///
/// `MediaSize` is intentionally not modeled: the TS predicate
/// (`reactiveCompact?.isWithheldMediaSizeError`) lives in unreleased
/// `services/compact/reactiveCompact.js`, and vercel-ai exposes no
/// typed media-rejection variant. Adding it now would be a
/// dead-code placeholder violating `coco-rs/CLAUDE.md` "No deprecated
/// code". When the predicate ships, add the variant alongside the
/// wire mapping in the same change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WithheldReason {
    /// Input + output exceeded the model context window. Recovery:
    /// reactive compaction (full / micro depending on config) →
    /// retry. TS parity: `query.ts:1085-1183` PTL recovery; multi-
    /// provider map: Anthropic `model_context_window_exceeded` finish
    /// reason + HTTP 400 `prompt_too_long`, OpenAI / Google / ByteDance
    /// HTTP 400 `context_length_exceeded`, all unified to
    /// `StopReason::ContextWindowExceeded`.
    PromptTooLong,
    /// Output budget hit before the response completed. Recovery:
    /// phase 1 escalate `max_output_tokens` to the model's
    /// `ModelInfo.max_output_tokens` (NOT a hard-coded 64k — that
    /// breaks GPT-4 (4k) and Haiku (1k); Finding N1), phase 2
    /// inject resume nudge up to `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`
    /// times. TS parity: `query.ts:1188-1255`.
    MaxOutputTokens,
}

/// Map a typed `StopReason` to a `WithheldReason`, or `None` if the
/// stream reached a stop reason that needs no recovery decision.
///
/// Single source of truth for "what does the recovery dispatcher do
/// with this stop reason?" — kept in this file so the in-stream loop
/// and the post-stream dispatcher agree without a sibling-module
/// invariant.
///
/// Note: `StopReason::ContentFilter` (refusal / `SAFETY` / `RECITATION`
/// / `content_filter`) is NOT a withhold-bucket — refusal is a policy
/// outcome, not a recoverable provider error. It maps to `None` and
/// falls through to the no-tool-calls terminal in `engine.rs`. See
/// `engine_stop_hooks::run_stop_hooks` for the matching
/// `isApiErrorMessage` short-circuit (Finding C3).
pub(crate) fn withhold_reason_for_stop(stop_reason: StopReason) -> Option<WithheldReason> {
    match stop_reason {
        StopReason::ContextWindowExceeded => Some(WithheldReason::PromptTooLong),
        StopReason::MaxTokens => Some(WithheldReason::MaxOutputTokens),
        StopReason::EndTurn
        | StopReason::StopSequence
        | StopReason::ToolUse
        | StopReason::ContentFilter
        | StopReason::Error
        | StopReason::Other => None,
    }
}

/// Per-turn stream terminal state. Three variants cover the four
/// terminal modes of the inner consume loop, with `PrematureClose`
/// also serving the cancel-arm path (cancellation is observed at the
/// caller via `self.cancel.is_cancelled()` — the cancel token is the
/// canonical signal, this enum is post-stream content only).
#[derive(Debug)]
pub(crate) enum StreamOutcome {
    /// `StreamEvent::Finish` arrived cleanly. Caller commits the
    /// reconstructed assistant message and dispatches recovery /
    /// tool execution on `stop_reason`.
    Finished {
        snapshot: Arc<AssistantTurnSnapshot>,
        usage: TokenUsage,
        stop_reason: StopReason,
    },
    /// `StreamEvent::Error` arrived mid-stream. Caller routes through
    /// [`crate::engine_recovery::QueryEngine::handle_stream_error`].
    /// `had_output` is `true` when any text / reasoning / tool-call was
    /// already emitted to the user before the error. It gates whether a
    /// retryable mid-stream error (e.g. an in-stream 429 delivered as an
    /// SSE error frame after HTTP 200) can be safely re-issued in place:
    /// re-issuing is only safe when nothing visible was emitted, else the
    /// retry would duplicate output.
    Errored { message: String, had_output: bool },
    /// Receiver closed without a Finish / Error event — either the
    /// channel ended early or the cancel-arm broke the inner loop.
    /// Caller falls through to the clean-turn success path with
    /// empty snapshot + `stop_reason: None`. Cancellation is
    /// disambiguated post-call via `self.cancel.is_cancelled()`.
    PrematureClose,
}

/// Per-turn stream consumption result. Accumulator fields are always
/// populated (possibly empty); `outcome` carries the terminal-state
/// payload. `reasoning_text` is consumed entirely inside
/// [`QueryEngine::consume_stream`] (only the `Finish` debug log reads
/// its length) and therefore is not exposed.
pub(crate) struct StreamConsumed {
    pub(crate) response_text: String,
    pub(crate) tool_order: Vec<String>,
    pub(crate) tool_buffers: HashMap<String, StreamingToolCallBuffer>,
    pub(crate) outcome: StreamOutcome,
}

impl QueryEngine {
    /// Phase 9 (TS parity `query.ts:855-1061`): drive the per-turn LLM
    /// stream to completion and return the typed [`StreamConsumed`].
    /// Caller branches on the four orthogonal terminal states (clean
    /// Finish / mid-stream Error / Cancel / premature channel close)
    /// via the four `Option` fields + `self.cancel.is_cancelled()`.
    ///
    /// Generic over the `StreamingHandle`'s closure type so the caller
    /// can pass its concrete `streaming_handle.as_mut()` without
    /// boxing. The bounds are the exact bounds `StreamingHandle`'s
    /// impl carries — copied verbatim from
    /// `core/tool-runtime/src/executor_streaming.rs:90-94`.
    ///
    /// Side effects: pushes synthetic-error tool_result rows + the
    /// matching assistant_msg on the streaming cancellation race
    /// (TS parity `query.ts:1015-1028`); emits `ToolUseStarted` for
    /// every successfully-prepared streaming tool call.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn consume_stream<F, Fut>(
        &self,
        rx: &mut tokio::sync::mpsc::Receiver<StreamEvent>,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        history: &mut MessageHistory,
        hook_tx_opt: Option<&tokio::sync::mpsc::Sender<coco_hooks::HookExecutionEvent>>,
        streaming_handle: &mut Option<StreamingHandle<F, Fut>>,
        streaming_ctx: Option<&Arc<ToolUseContext>>,
        streaming_model_index: &mut usize,
        state_tracker: &SessionStateTracker,
        turn_id: &str,
        _consts: &LoopConstants,
        services: &LoopServices,
        acc: &mut LoopAccumulator,
        turn_state: &LoopTurnState,
    ) -> StreamConsumed
    where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send + 'static,
    {
        // Accumulate stream state. `tool_order` preserves the order tool
        // calls first appeared (by `ToolInputStart`) so the downstream
        // exec path keeps the same ordering contract as the blocking path.
        //
        // `response_text` and `reasoning_text` are presentation-only
        // accumulators driven from `StreamEvent::{TextDelta, ReasoningDelta}`:
        //
        // - `response_text` feeds the Stop hook's `last_assistant_message`
        //   input, log fields, and `QueryResult.response_text`.
        // - `reasoning_text` feeds a log field.
        //
        // The history-bearing assistant content is reconstructed from
        // `event.snapshot` at `StreamEvent::Finish` — this is the path
        // that preserves per-part `provider_metadata` (Gemini
        // `thoughtSignature`, Anthropic `signature`, OpenAI
        // `encrypted_content`). See `docs/coco-rs/streaming-metadata-roundtrip-plan.md`.
        let mut response_text = String::new();
        let mut reasoning_text = String::new();
        let mut tool_order: Vec<String> = Vec::new();
        let mut tool_buffers: HashMap<String, StreamingToolCallBuffer> = HashMap::new();
        // Default to `PrematureClose`: an `rx.recv() == None` or the
        // cancel-select arm both leave this untouched and break out of
        // the loop. `Finish` / `Error` event arms overwrite with the
        // appropriate variant before breaking.
        let mut outcome = StreamOutcome::PrematureClose;

        loop {
            let event = tokio::select! {
                _ = self.cancel.cancelled() => {
                    // Cancellation mid-stream: close the receiver
                    // (no more events delivered) and fall through to
                    // the caller's `is_cancelled()` check which
                    // returns `Ok(QueryResult { cancelled: true })`.
                    // With streaming_tool_execution enabled, the
                    // StreamingHandle's JoinSet aborts any
                    // inflight safe tools when dropped (transitively
                    // via streaming_handle going out of scope as the
                    // outer `run_session_loop` unwinds). We can't
                    // `drop(rx)` here because we hold `&mut`; closing
                    // is the equivalent termination signal.
                    rx.close();
                    break;
                }
                ev = rx.recv() => ev,
            };
            let Some(event) = event else {
                // Channel closed without Finish/Error — treat as a premature
                // end. Keep whatever content we accumulated; callers fall
                // through to the empty-tool_calls exit below.
                break;
            };

            match event {
                StreamEvent::TextDelta { text } => {
                    response_text.push_str(&text);
                    let _ = crate::emit::emit_stream(
                        event_tx,
                        crate::AgentStreamEvent::TextDelta {
                            turn_id: turn_id.to_string(),
                            delta: text,
                        },
                    )
                    .await;
                }
                StreamEvent::ReasoningDelta { text } => {
                    reasoning_text.push_str(&text);
                    let _ = crate::emit::emit_stream(
                        event_tx,
                        crate::AgentStreamEvent::ThinkingDelta {
                            turn_id: turn_id.to_string(),
                            delta: text,
                        },
                    )
                    .await;
                }
                StreamEvent::ReasoningEnd { .. } => {
                    // No-op: the snapshot accumulator at the
                    // coco-inference layer captures the signature
                    // per-segment and surfaces it on
                    // `StreamEvent::Finish.snapshot` for full
                    // multi-reasoning fidelity (see plan v6).
                }
                StreamEvent::ToolCallStart { id, tool_name } => {
                    if !tool_buffers.contains_key(&id) {
                        tool_order.push(id.clone());
                    }
                    // Open a pre-queue "streaming" row in the TUI activity strip
                    // so arguments can render as they arrive. The `call_id` here
                    // matches the later `ToolUseQueued` (both use this stream
                    // `id`), which upgrades the same row with the finalized input.
                    let _ = crate::emit::emit_tui(
                        event_tx,
                        coco_types::TuiOnlyEvent::ToolCallStreamStart {
                            call_id: id.clone(),
                            name: tool_name.clone(),
                        },
                    )
                    .await;
                    tool_buffers.insert(
                        id.clone(),
                        StreamingToolCallBuffer {
                            tool_name,
                            input_json: String::new(),
                            complete: false,
                        },
                    );
                }
                StreamEvent::ToolCallDelta { id, delta } => {
                    if let Some(buf) = tool_buffers.get_mut(&id) {
                        buf.input_json.push_str(&delta);
                    }
                    // Forward the partial JSON fragment to the TUI for the live
                    // "typing" preview. UI-only — the SDK gets the complete,
                    // re-assembled input at `ToolUseQueued`. The TUI coalesces
                    // these per call_id, so per-token emission is cheap.
                    let _ = crate::emit::emit_tui(
                        event_tx,
                        coco_types::TuiOnlyEvent::ToolCallDelta { call_id: id, delta },
                    )
                    .await;
                }
                StreamEvent::ToolCallEnd { id } => {
                    if let Some(buf) = tool_buffers.get_mut(&id) {
                        buf.complete = true;
                    }
                    // Streaming mode: parse the freshly-completed
                    // input, run full per-tool preparation
                    // (validate → pre-hook → permission →
                    // re-validate), and feed the resulting plan
                    // to the StreamingHandle. Safe tools start
                    // executing immediately via tokio::spawn;
                    // unsafe tools queue for commit_flush.
                    //
                    // ── I1 invariant fix ──
                    // The preparer's early-error paths push
                    // synthetic tool_result rows to history
                    // directly (non-streaming behaviour). In
                    // streaming mode, the assistant message
                    // hasn't been committed yet — it lands at the
                    // `Finish` arm below. A naive inline push
                    // produces history of:
                    //   N:   user/tool_result (synthetic error)
                    //   N+1: assistant/tool_use(s)
                    // ...which violates Anthropic's strict
                    // tool_use/tool_result adjacency.
                    //
                    // Capture the pre-call length, then drain any
                    // pushes after preparation. Successful prep
                    // makes no pushes (the plan is fed to the
                    // handle); failed prep pushes the synthetic
                    // error, which we re-wrap as an
                    // `EarlyOutcome` so `commit_flush` surfaces
                    // it AFTER the assistant message lands.
                    if let (Some(handle), Some(ctx_arc)) =
                        (streaming_handle.as_mut(), streaming_ctx)
                        && let Some(buf) = tool_buffers.get(&id)
                        && buf.complete
                    {
                        let parsed_input = crate::tool_input_parse::parse_tool_arguments_or_empty(
                            &buf.input_json,
                            &buf.tool_name,
                        );
                        let input = crate::tool_input_normalizer::normalize_observable_tool_input(
                            &buf.tool_name,
                            parsed_input,
                            crate::tool_input_normalizer::ToolInputNormalizationContext {
                                cwd: None,
                            },
                        );
                        let tcp = ToolCallPart {
                            tool_call_id: id.clone(),
                            tool_name: buf.tool_name.clone(),
                            input,
                            provider_executed: None,
                            invalid: false,
                            invalid_reason: None,
                            provider_metadata: None,
                        };
                        let slice = std::slice::from_ref(&tcp);
                        let mut deferred_tool_completions =
                            crate::helpers::DeferredToolCompletionBuffer::new(
                                *streaming_model_index,
                            );
                        let mut prep_args = crate::tool_call_preparer::PendingToolPreparation {
                            event_tx,
                            history: &mut *history,
                            ctx: ctx_arc.as_ref(),
                            tool_calls: slice,
                            tools: &self.tools,
                            hooks: self.hooks.as_ref(),
                            orchestration_ctx: self.orchestration_ctx(),
                            hook_tx_opt,
                            permission_denials: &mut acc.permission_denials,
                            state_tracker,
                            permission_bridge: self.permission_bridge.as_ref(),
                            session_id: &self.config.session_id,
                            cancel: &self.cancel,
                            auto_mode_state: self.auto_mode_state.as_ref(),
                            denial_tracker: self.denial_tracker.as_ref(),
                            model_runtimes: &self.model_runtimes,
                            auto_mode_rules: &self.auto_mode_rules,
                            completion_event_mode: crate::helpers::ToolCompletionEventMode::Defer,
                            deferred_tool_completions: Some(&mut deferred_tool_completions),
                        };
                        let mut permission_aborted = false;
                        let prep_result = crate::tool_call_preparer::prepare_one_pending_tool_call(
                            &mut prep_args,
                            &tcp,
                            &mut permission_aborted,
                        )
                        .await;
                        drop(prep_args);
                        if permission_aborted {
                            self.cancel.cancel();
                        }
                        *streaming_model_index = deferred_tool_completions.next_model_index();
                        let deferred_outcomes = deferred_tool_completions.into_outcomes();

                        match prep_result {
                            Some((pending, ctx)) => {
                                debug_assert!(
                                    deferred_outcomes.is_empty(),
                                    "preparation succeeded but staged deferred completions"
                                );
                                let _ = crate::emit::emit_stream(
                                    event_tx,
                                    crate::AgentStreamEvent::ToolUseStarted {
                                        call_id: pending.tool_use_id.clone(),
                                        name: pending.tool.name().to_string(),
                                        batch_id: None,
                                    },
                                )
                                .await;
                                let model_index = *streaming_model_index;
                                *streaming_model_index += 1;
                                handle.feed_plan(ToolCallPlan::Runnable(PreparedToolCall {
                                    tool_use_id: pending.tool_use_id,
                                    tool_id: pending.tool.id(),
                                    tool: pending.tool,
                                    parsed_input: pending.input,
                                    is_concurrency_safe: pending.is_concurrency_safe,
                                    model_index,
                                    permission_resolution_detail: ctx.permission_resolution_detail,
                                    approval_feedback: ctx.approval_feedback,
                                }));
                            }
                            None if !deferred_outcomes.is_empty() => {
                                if self.cancel.is_cancelled() {
                                    let mut content_parts = Vec::new();
                                    if !response_text.is_empty() {
                                        content_parts.push(AssistantContentPart::Text(TextPart {
                                            text: response_text.clone(),
                                            provider_metadata: None,
                                        }));
                                    }
                                    content_parts.push(AssistantContentPart::ToolCall(tcp.clone()));
                                    crate::history_sync::history_push_and_emit(
                                        history,
                                        Message::Assistant(AssistantMessage {
                                            message: LlmMessage::Assistant {
                                                content: content_parts
                                                    .into_iter()
                                                    .map(crate::helpers::convert_to_assistant_content)
                                                    .collect(),
                                                provider_options: None,
                                            },
                                            uuid: uuid::Uuid::new_v4(),
                                            model: services.current_model_id(),
                                            stop_reason: Some(StopReason::ToolUse),
                                            usage: None,
                                            cost_usd: None,
                                            request_id: None,
                                            api_error: None,
                                        }),
                                        event_tx,
                                    )
                                    .await;
                                    for outcome in deferred_outcomes {
                                        for msg in outcome.ordered_messages {
                                            crate::history_sync::history_push_and_emit(
                                                history, msg, event_tx,
                                            )
                                            .await;
                                        }
                                    }
                                } else {
                                    for outcome in deferred_outcomes {
                                        handle.feed_plan(ToolCallPlan::EarlyOutcome(outcome));
                                    }
                                }
                            }
                            None => {
                                // Rare: prep returned None with
                                // no captured messages. Drop
                                // silently — there is no
                                // model-visible result to pair.
                            }
                        }
                    }
                }
                StreamEvent::Finish {
                    usage,
                    stop_reason,
                    snapshot,
                    ..
                } => {
                    // `%stop_reason` renders the full `FinishReason` —
                    // its `Display` annotates the provider-original raw
                    // when it differs from the projection (e.g.
                    // `other(compaction)`). This debug line is the one
                    // place the streaming path surfaces `raw`; downstream
                    // carries only the `.unified` projection.
                    tracing::debug!(
                        turn = turn_state.turn,
                        turn_id = %turn_id,
                        stop_reason = %stop_reason,
                        tokens_in = usage.input_tokens.total,
                        tokens_out = usage.output_tokens.total,
                        cache_read = usage.input_tokens.cache_read,
                        cache_creation = usage.input_tokens.cache_write,
                        text_chars = response_text.len(),
                        reasoning_chars = reasoning_text.len(),
                        tool_call_count = tool_order.len(),
                        "LLM stream finished"
                    );
                    // Report the authoritative outcome to the wire dumper:
                    // an abnormal stop (content_filter / max_tokens) is a
                    // failure, a clean finish is a success.
                    if let Some(rec) = turn_state.wire_recorder.as_ref() {
                        rec.finish(coco_wire_dump::WireOutcome::from_is_normal(
                            stop_reason.unified.is_normal(),
                        ));
                    }
                    outcome = StreamOutcome::Finished {
                        snapshot,
                        usage,
                        // Project to the behavioral enum — `raw` was just
                        // logged above and is not needed past this seam.
                        stop_reason: stop_reason.unified,
                    };
                    break;
                }
                StreamEvent::Error { message, .. } => {
                    // Nothing committed to the user yet ⇒ a retryable
                    // mid-stream error can be re-issued in place without
                    // duplicating visible output. `tool_order` empty also
                    // means no streaming tool started executing.
                    let had_output = !response_text.is_empty()
                        || !reasoning_text.is_empty()
                        || !tool_order.is_empty();
                    warn!(
                        turn = turn_state.turn,
                        turn_id = %turn_id,
                        error = %message,
                        text_chars = response_text.len(),
                        tool_call_count = tool_order.len(),
                        had_output,
                        "LLM stream errored"
                    );
                    if let Some(rec) = turn_state.wire_recorder.as_ref() {
                        rec.finish(coco_wire_dump::WireOutcome::Failure);
                    }
                    outcome = StreamOutcome::Errored {
                        message,
                        had_output,
                    };
                    break;
                }
            }
        }

        // `reasoning_text` is consumed only by the in-loop Finish
        // debug log; intentionally dropped here.
        let _ = reasoning_text;
        StreamConsumed {
            response_text,
            tool_order,
            tool_buffers,
            outcome,
        }
    }

    /// Cancellation epilogue: drain any plans the streaming handle had
    /// staged (their `pending_early` rows are synthetic-error
    /// `tool_result` messages parked for the I1-ordered commit),
    /// synthesize the matching `tool_use` assistant message so the
    /// transcript keeps Anthropic's strict tool_use/tool_result
    /// adjacency, push the drained outcomes, then write the canonical
    /// user-cancel marker.
    ///
    /// Called from `run_session_loop` immediately after
    /// `consume_stream` returns when `self.cancel.is_cancelled()` is
    /// true. Caller `continue`s the outer turn loop after this
    /// returns so the top-of-loop cancel check builds the proper
    /// `QueryResult { cancelled: true }`.
    ///
    /// TS parity: `query.ts:1015-1028`
    /// (`yieldMissingToolResultBlocks` after abort).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn cancel_epilogue<F, Fut>(
        &self,
        streaming_handle: &mut Option<StreamingHandle<F, Fut>>,
        tool_order: &[String],
        tool_buffers: &HashMap<String, StreamingToolCallBuffer>,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        services: &LoopServices,
        _consts: &LoopConstants,
    ) where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send + 'static,
    {
        let mut had_tool_use = false;
        if let Some(handle) = streaming_handle.take() {
            let discarded = handle.discard().await;
            let early: Vec<_> = discarded
                .into_iter()
                .filter(|o| !o.ordered_messages.is_empty())
                .collect();
            if !early.is_empty() {
                had_tool_use = true;
                let kept_ids: std::collections::HashSet<&String> =
                    early.iter().map(|o| &o.tool_use_id).collect();
                let synth_parts: Vec<coco_inference::TurnPart> = tool_order
                    .iter()
                    .filter(|id| kept_ids.contains(*id))
                    .filter_map(|id| tool_buffers.get(id).map(|buf| (id, buf)))
                    .map(|(id, buf)| {
                        coco_inference::TurnPart::ToolCall(coco_inference::ToolCallSegment {
                            id: id.clone(),
                            tool_name: buf.tool_name.clone(),
                            input_json: buf.input_json.clone(),
                            provider_executed: None,
                            dynamic: None,
                            is_input_complete: buf.complete,
                            is_complete: false,
                            provider_metadata: None,
                            invalid: false,
                            invalid_reason: None,
                        })
                    })
                    .collect();
                let synth_snapshot = AssistantTurnSnapshot { parts: synth_parts };
                let (content_parts, _) = crate::engine::assistant_content_from_snapshot(
                    &synth_snapshot,
                    crate::tool_input_normalizer::ToolInputNormalizationContext { cwd: None },
                );
                if !content_parts.is_empty() {
                    let assistant_msg = Message::Assistant(AssistantMessage {
                        message: LlmMessage::Assistant {
                            content: content_parts
                                .into_iter()
                                .map(crate::helpers::convert_to_assistant_content)
                                .collect(),
                            provider_options: None,
                        },
                        uuid: uuid::Uuid::new_v4(),
                        model: services.current_model_id(),
                        stop_reason: None,
                        usage: None,
                        cost_usd: None,
                        request_id: None,
                        api_error: None,
                    });
                    crate::history_sync::history_push_and_emit(history, assistant_msg, event_tx)
                        .await;
                }
                for outcome in early {
                    for msg in outcome.ordered_messages {
                        crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
                    }
                }
            }
        }
        // Mid-stream cancel: tool calls may have been synthesized into
        // history above (`had_tool_use` tracks the engine's
        // authoritative view), so `for_tool_use` is decided once here
        // and stored on the typed marker — downstream renders read the
        // field rather than recomputing from running-tool state. See
        // `engine-tui-unified-transcript-plan.md` §7.2.
        //
        // Steering exception: on a submit-interrupt the queued user message
        // provides continuity, so skip the redundant standalone marker (TS
        // `query.ts:1046` parity). The per-tool interrupt `tool_result`s
        // synthesized above are kept (required for tool_use pairing).
        if crate::history_sync::is_steering_interrupt(self.turn_abort.reason()) {
            tracing::debug!(
                "finalize_user_cancel: skipped standalone marker (submit-interrupt steering)"
            );
        } else {
            crate::history_sync::finalize_user_cancel(
                history,
                /*in_flight_tool_calls*/ had_tool_use,
                event_tx,
            )
            .await;
        }
    }
}
