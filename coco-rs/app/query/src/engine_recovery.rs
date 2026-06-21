//! Post-stream recovery dispatcher.
//!
//! Owns the decision tree that runs when [`crate::engine_stream_consume`]
//! signals a [`WithheldReason`]: the model produced an `assistant_msg`
//! whose stop reason maps to a recoverable bucket, and we now decide
//! whether to retry (compaction / output-budget escalation /
//! resume-nudge) or surface the synthetic `api_error` message and
//! fall through to the no-tool-calls terminal.
//!
//! ## Multi-provider adaptation
//!
//! Every provider's signal is pre-mapped into the typed
//! [`coco_messages::StopReason`] at the `vercel-ai-*` adapter seam,
//! then collapses to a [`WithheldReason`] in
//! `engine_stream_consume::withhold_reason_for_stop`. The dispatcher
//! matches the typed enum — never the raw provider string.
//!
//! Reads [`coco_config::ModelInfo::max_output_tokens_escalate`] — a
//! per-model opt-in ceiling. When unset, Phase-1 escalate is
//! **disabled** for that model and recovery jumps straight to the
//! multi-turn resume nudge (Phase-2). Escalate is a derived property
//! of `ModelInfo` + the per-turn `transition` field, not a stateful
//! slot.
//!
//! `CONTEXT_COLLAPSE` / `HISTORY_SNIP` are intentionally out of scope
//! per the multi-provider architecture.
//!
//! ## Withhold semantics (Finding C4 / C22)
//!
//! The synthetic api_error message is withheld **during** the stream
//! and only yielded on recovery exhaustion. The previous behavior
//! (pre-commit-3) pushed the synthetic immediately at every stop
//! reason, producing phantom error messages on the happy compact-retry
//! path. The dispatcher fixes this by:
//!
//! - **`PromptTooLong`**: pushes `assistant_msg` (partial response
//!   remains visible) and triggers reactive compaction; no synthetic
//!   push. `do_reactive_compact`'s circuit breaker may no-op when
//!   compaction has already fired recently — the next turn surfaces
//!   the same condition naturally without a phantom message.
//! - **`MaxOutputTokens` escalate**: pushes neither the partial
//!   `assistant_msg` (would be reused by the retry's bigger budget)
//!   nor the synthetic (would precede a successful retry's clean
//!   response).
//! - **`MaxOutputTokens` recover**: pushes `assistant_msg` (partial
//!   output visible to the model on the resume turn) + the meta
//!   "resume" nudge; still no synthetic until exhaustion.
//! - **`MaxOutputTokens` exhausted**: pushes `assistant_msg` + the
//!   synthetic, then falls through. This is the only path that emits
//!   the synthetic.
//!
//! ## ContentFilter
//!
//! Not a [`WithheldReason`] — refusal is a terminal policy decision,
//! not a recoverable provider error. ContentFilter is handled inline
//! at the dispatch site in `engine.rs` (push assistant + synthetic,
//! fall through). The [`crate::engine_stream_consume::withhold_reason_for_stop`]
//! function returns `None` for it on purpose, and the
//! `isApiErrorMessage` short-circuit in commit 5 prevents Stop hooks
//! from cycling on the refusal.

use coco_inference::ModelRuntimeSnapshot;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::StopReason;
use coco_tool_runtime::PreparedToolCall;
use coco_tool_runtime::RunOneRuntime;
use coco_tool_runtime::StreamingHandle;
use coco_tool_runtime::UnstampedToolCallOutcome;
use tracing::warn;

use crate::config::ContinueReason;
use crate::config::MAX_OUTPUT_TOKENS_RECOVERY_LIMIT;
use crate::engine::QueryEngine;
use crate::engine_helpers::emit_model_fallback_notice;
use crate::engine_helpers::extract_streaming_result_text;
use crate::engine_loop_state::LoopServices;
use crate::engine_loop_state::LoopTurnState;
use crate::engine_stream_consume::WithheldReason;

#[cfg(test)]
#[path = "engine_recovery.test.rs"]
mod tests;

/// Outcome of [`QueryEngine::handle_stream_error`]: tell the caller
/// whether to `continue` the outer loop or surface a fatal error.
#[derive(Debug)]
pub(crate) enum StreamErrorOutcome {
    /// Recoverable error — `turn_state` / `services` already mutated
    /// in place; caller `continue`s the outer loop.
    Continue,
    /// Unrecoverable error — caller returns this as the session
    /// loop's `Err(_)`.
    Bail(coco_error::BoxedError),
}

pub(crate) struct OpenedTurnStream {
    pub(crate) rx: tokio::sync::mpsc::Receiver<coco_inference::StreamEvent>,
    pub(crate) token: coco_inference::ModelCallHandle,
    pub(crate) snapshot: ModelRuntimeSnapshot,
}

/// Outcome of [`QueryEngine::handle_context_overflow`]: tell the caller
/// whether reactive compaction made progress (caller retries) or
/// exhausted (caller pushes synthetic api_error + terminates).
///
/// Finding **R1**. Without this typed signal, the Rust loop spun until
/// budget exhaustion because `do_reactive_compact`'s internal
/// circuit-breaker no-op'd silently and the dispatcher always returned
/// `Continue(ReactiveCompactRetry)`.
#[derive(Debug)]
pub(crate) enum ContextOverflowOutcome {
    /// Compaction freed at least one token; caller continues with this
    /// transition (`ReactiveCompactRetry` today, but the enum keeps a
    /// transition argument so future variants like `CollapseDrainRetry`
    /// can slot in here without breaking callers).
    Compacted(ContinueReason),
    /// Compaction made no progress (circuit-breaker tripped or zero
    /// freed). Caller MUST push a synthetic api_error message and route
    /// to a terminal exit — from the Finish/recovery path that means
    /// `RecoveryDisposition::TerminateExhausted` + fall through to the
    /// no-tool-calls terminal (the C3 guard then fires StopFailure and
    /// the engine emits `stop_reason` from the api_error code); from
    /// the stream-open / mid-stream sites that means [`StreamErrorOutcome::Bail`]
    /// with a `ContextWindowExceeded` `PlainError`.
    Exhausted,
}

/// Capacity-error classification carrying the provider-reported
/// `retry_after_ms` when available. Used by the shared rate-limit
/// observation recorder (Finding **A1**).
#[derive(Debug, Clone, Copy)]
struct CapacityKind {
    retry_after_ms: Option<i64>,
}

/// Extract a [`CapacityKind`] from a typed [`coco_inference::InferenceError`]
/// classification when the variant is `Overloaded` / `RateLimited`.
/// Returns `None` for any other variant; the stream-open path may
/// still detect capacity via [`crate::engine_helpers::is_capacity_error_message`]
/// string fallback after this returns `None`.
fn capacity_kind_from(classified: Option<&coco_inference::InferenceError>) -> Option<CapacityKind> {
    match classified? {
        coco_inference::InferenceError::Overloaded { retry_after_ms, .. }
        | coco_inference::InferenceError::RateLimited { retry_after_ms, .. } => {
            Some(CapacityKind {
                retry_after_ms: *retry_after_ms,
            })
        }
        _ => None,
    }
}

/// Max consecutive in-place retries for a mid-stream capacity error
/// (in-stream 429 / 529 delivered as an SSE error frame after HTTP 200,
/// with no output emitted). Matches the handshake cap
/// (`coco_inference::RetryConfig` capacity bound) so both throttle
/// channels surface the error after the same number of attempts.
pub(crate) const MAX_MIDSTREAM_CAPACITY_RETRIES: i32 = 3;

/// Backoff before an in-place mid-stream capacity retry. Honors the
/// server-supplied `retry-after` when present (and positive); otherwise
/// exponential `500ms · 2^(attempt-1)` capped at 8s. Deliberately small
/// and bounded — the attempt count is capped at
/// [`MAX_MIDSTREAM_CAPACITY_RETRIES`].
fn midstream_capacity_backoff(retry_after_ms: Option<i64>, attempt: i32) -> std::time::Duration {
    if let Some(ms) = retry_after_ms
        && ms > 0
    {
        return std::time::Duration::from_millis(ms as u64);
    }
    let shift = (attempt - 1).clamp(0, 4) as u32;
    let exp_ms = 500_i64.saturating_mul(1_i64 << shift).min(8_000);
    std::time::Duration::from_millis(exp_ms as u64)
}

/// Minimum reserved output budget used by the pre-API gate when
/// computing `context_window - reserved_output`. The recovery
/// dispatcher and [`QueryEngineConfig::default`] (via
/// [`crate::config::DEFAULT_CONTEXT_WINDOW`]) share the same fallback
/// pair so the threshold math stays self-consistent.
const MIN_RESERVED_OUTPUT: i64 = 1_024;

/// Pre-API gate outcome (Finding **C15**). Lives next to the recovery
/// dispatcher because it sits on the same axis (preventing /
/// rescuing from context-overflow), and the implementation routes
/// to the same `build_abnormal_stop_api_error_message` synthesis.
///
/// Two-variant by design: the previous `SkipPostCompact` variant was
/// behaviorally identical to `Proceed` (caller did nothing in either
/// arm) and only existed as a tag for the post-compact iteration; that
/// info now lives in a `tracing::debug!` field inside
/// [`QueryEngine::check_blocking_limit`] (Finding **R10**).
#[derive(Debug)]
pub(crate) enum BlockingLimitDecision {
    /// Estimated history fits within the model's context window minus
    /// the reserved output budget — OR the gate was intentionally
    /// skipped (post-compact retry, forked compact/session-memory
    /// agent). Caller proceeds to `query_stream`.
    Proceed,
    /// Hard over-limit — pushing more history at the API would 4xx.
    /// Caller pushes the synthetic api_error and returns early with
    /// `stop_reason = "blocking_limit"`.
    Block {
        estimated_tokens: i64,
        context_window: i64,
    },
}

/// Whether the given query-source label corresponds to a forked
/// compact / session-memory agent that exists specifically to shrink
/// oversized history. C15 must skip the pre-API gate for these so the
/// fork actually reaches the provider (Finding **R5**).
///
/// Labels `session_memory_auto`, `session_memory_manual`, and
/// `extract_memories` also operate on the parent's oversized state.
/// `prompt_suggestion` / `agent_summary` / `side_question` are NOT
/// in this set — those forks are post-turn, run on already-fitting
/// history, and benefit from the gate.
fn is_forked_compact_or_session_memory_source(qs: &str) -> bool {
    matches!(
        qs,
        "compact"
            | "session_memory"
            | "session_memory_auto"
            | "session_memory_manual"
            | "extract_memories"
    )
}

/// Outcome of running the post-stream recovery dispatcher.
///
/// The dispatcher takes ownership of the partial `assistant_msg` and
/// performs every push into [`MessageHistory`] internally so that
/// non-recovery code paths (the `else` arms in `run_session_loop`)
/// never accidentally re-push the same message.
#[derive(Debug)]
pub(crate) enum RecoveryDisposition {
    /// Recovery applied; caller should write `turn_state.transition =
    /// Some(reason)` and `continue` the outer loop. The dispatcher has
    /// already pushed whichever subset of {`assistant_msg`,
    /// resume-nudge meta message} the recovery branch needs.
    Continue(ContinueReason),
    /// Recovery exhausted (max-output-tokens retry budget hit). The
    /// dispatcher has pushed `assistant_msg` + the synthetic api_error
    /// message; caller should fall through to the no-tool-calls
    /// terminal so Stop hooks (with the `isApiErrorMessage` guard
    /// from commit 5) can finalize the turn cleanly.
    TerminateExhausted,
}

/// Per-call `max_tokens` derivation. Single source of truth for what the
/// LLM API call's `max_output_tokens` parameter should be.
///
/// Resolution:
/// 1. If `turn_state.transition == Some(MaxOutputTokensEscalate)` AND the
///    active model defines [`coco_config::ModelInfo::max_output_tokens_escalate`],
///    return that escalate ceiling — Phase-1 retry uses it for one turn.
/// 2. Otherwise return `None` — the inference layer falls through to
///    `ModelInfo.max_output_tokens` (the model's baseline cap).
///
/// `None` from a missing escalate field on the escalate path is a no-op
/// — it means the model didn't opt in, so Phase-1 was never triggered to
/// begin with (the recovery dispatcher's gate fires before this is read).
///
/// This replaces the legacy 3-source resolution
/// (`turn_state.max_tokens_override.or(self.config.max_tokens)` — both
/// fields removed in this refactor): per-model `ModelInfo` is the only
/// knob; the global `QueryEngineConfig.max_tokens` was an Anthropic-only
/// residue that couldn't survive the `ModelRole` × multi-LLM swap
/// surface.
pub(crate) fn effective_max_tokens(
    active_snapshot: &ModelRuntimeSnapshot,
    turn_state: &LoopTurnState,
) -> Option<i64> {
    let escalating = matches!(
        turn_state.transition,
        Some(ContinueReason::MaxOutputTokensEscalate)
    );
    if escalating {
        active_snapshot
            .model_info
            .as_ref()
            .and_then(|info| info.max_output_tokens_escalate)
            .map(i64::from)
    } else {
        None
    }
}

impl QueryEngine {
    /// Cross-provider housekeeping after [`ModelRuntime::advance`]
    /// reports a fallback switch.
    ///
    /// **Finding N2**: when fallback switches providers (Anthropic →
    /// OpenAI, Google → Anthropic, etc.) the new runtime slot carries
    /// its own [`coco_inference::cache_detection::CacheBreakDetector`].
    /// If the new slot was active earlier in this session
    /// (e.g. probe scenarios), its detector may hold stale prompt-state
    /// hashes from before the switch. `cache_break_reset()` clears
    /// them so the post-switch request establishes a fresh baseline
    /// instead of false-positive-firing.
    ///
    /// Resetting unconditionally (even within-provider switches)
    /// is the conservative choice — the cost is one extra Mutex lock
    /// per advance, and within-provider switches are already rare.
    /// The provider-change comparison drives the `info!` log only,
    /// for ops visibility.
    ///
    /// **Finding N3 (resolved — no engine-layer action needed)**: an
    /// earlier draft flagged "strip Anthropic thinking signatures when
    /// crossing FROM Anthropic to a non-Anthropic provider." Auditing
    /// the `vercel-ai-*` adapters confirms this is already defensive
    /// at the wire seam:
    ///
    /// - `vercel-ai-anthropic::convert_to_anthropic_messages` checks
    ///   for `provider_metadata.anthropic.{signature,redactedData}`
    ///   and drops the reasoning block entirely (with a `Warning`)
    ///   when neither is present.
    /// - `vercel-ai-openai::convert_to_chat_messages` skips
    ///   `AssistantContentPart::Reasoning` outright in the Chat API.
    /// - `vercel-ai-openai::convert_to_responses_input` reads only
    ///   `rp.text`; never inspects `provider_metadata`.
    /// - `vercel-ai-openai-compatible::convert_to_chat_messages`
    ///   reads only `rp.text`; never inspects `provider_metadata`.
    /// - `vercel-ai-google::convert_to_google_generative_ai_messages`
    ///   extracts only `google.thoughtSignature`; ignores
    ///   `anthropic.signature`.
    ///
    /// Each adapter namespaces `provider_metadata` reads by provider
    /// key, so foreign-key signatures never reach a wire body that
    /// would reject them. The intra-Anthropic cross-model case (same
    /// provider, signatures bound to a specific model) is explicitly
    /// out of scope per the workspace `feedback_no_ant_gates`.
    pub(crate) async fn post_advance_side_effects(
        &self,
        original_provider: &str,
        services: &LoopServices,
    ) {
        let snapshot = services.snapshot();
        let new_provider = snapshot.provider.as_str();
        if new_provider != original_provider {
            tracing::info!(
                from_provider = original_provider,
                to_provider = new_provider,
                "cross-provider fallback advance: resetting CacheBreakDetector",
            );
        }
        services.reset_active_cache_break_detector().await;
    }

    /// Pre-API blocking-limit check (Finding **C15**). Computes an
    /// estimated token count for the prompt that will be sent and
    /// compares it against the active model's context window minus the
    /// reserved output budget. When over the threshold, callers push
    /// a synthetic api_error message and exit the loop with
    /// `stop_reason = "blocking_limit"` rather than letting the
    /// request hit the API and 4xx.
    ///
    /// Multi-provider: reads `context_window` from the **active**
    /// client's [`coco_config::ModelInfo`] — when plan-mode swaps to
    /// a smaller Plan model (e.g. Haiku at 200k vs Opus at 1M) the
    /// gate scales with the swap, not the user's headline model.
    ///
    /// `effective_max_tokens` is the value the caller will actually pass
    /// as the API's max_output_tokens parameter — derived from
    /// [`effective_max_tokens`] (the free function), which reads the
    /// active model's [`coco_config::ModelInfo::max_output_tokens_escalate`]
    /// during a `MaxOutputTokensEscalate` retry and `None` otherwise.
    /// When present and > 0 it drives the reserved budget directly — the
    /// gate then matches what the provider will enforce
    /// (`prompt + max_tokens ≤ context_window`). When `None`, the gate
    /// reads the model's baseline `max_output_tokens` from the active
    /// client's `ModelInfo`, then falls back to
    /// `max(MIN_RESERVED_OUTPUT, context_window / 10)` only if no
    /// `ModelInfo` is wired (test paths). This way the gate matches the
    /// provider's `prompt + max_tokens ≤ context_window` enforcement on
    /// production paths regardless of escalate state (Finding **R8**).
    ///
    /// Returns [`BlockingLimitDecision::Proceed`] when the request fits
    /// or the gate is intentionally skipped:
    /// * Previous iteration triggered a reactive-compact retry —
    ///   re-blocking would deadlock the recovery (compact → block →
    ///   compact → …).
    /// * Query source is a forked compact/session-memory agent — those
    ///   forks exist to shrink the oversized history and must reach
    ///   the provider with their original input (Finding **R5**).
    pub(crate) fn check_blocking_limit(
        &self,
        history: &MessageHistory,
        active_snapshot: &ModelRuntimeSnapshot,
        turn_state: &LoopTurnState,
        effective_max_tokens: Option<i64>,
    ) -> BlockingLimitDecision {
        // Skip the gate when the previous iteration triggered a
        // reactive-compact retry — the just-compacted history is
        // what we want to send, and re-blocking would deadlock the
        // recovery (compact → block → compact → …).
        if matches!(
            turn_state.transition,
            Some(ContinueReason::ReactiveCompactRetry)
        ) {
            tracing::debug!("C15 gate skipped: post-compact retry iteration");
            return BlockingLimitDecision::Proceed;
        }

        // Finding **R5** — forked agents whose entire purpose is to
        // shrink oversized history must reach the provider with their
        // input intact. `forked_agent::ForkLabel::as_str()` and the
        // user's [`QueryEngineConfig::query_source_override`] supply
        // matching labels; the engine's own `query_source_label()`
        // collapses to these for fork-built engines.
        let qs = self.query_source_label();
        if is_forked_compact_or_session_memory_source(qs) {
            tracing::debug!(
                query_source = qs,
                "C15 gate skipped: forked compact / session-memory agent",
            );
            return BlockingLimitDecision::Proceed;
        }

        let model_info = active_snapshot.model_info.as_ref();
        let context_window = model_info
            .map(|info| i64::from(info.context_window))
            .unwrap_or(crate::config::DEFAULT_CONTEXT_WINDOW);
        let model_baseline_max_output = model_info.map(|info| i64::from(info.max_output_tokens));

        let estimated_tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());

        // Finding **R8** — reserved budget tracks what the provider will
        // actually enforce as `prompt + max_tokens ≤ window`. Three
        // tiers, most-specific first:
        //
        // 1. Phase-1 escalate retry — `effective_max_tokens = Some(N)`
        //    (the escalate ceiling for this one turn). Use it directly.
        // 2. ModelInfo baseline — the model's `max_output_tokens` is
        //    what the next non-escalate call will use. Most accurate
        //    threshold on production paths.
        // 3. Fallback heuristic — only when no ModelInfo is wired
        //    (mocked clients / test fixtures).
        let reserved_output = effective_max_tokens
            .filter(|v| *v > 0)
            .or(model_baseline_max_output)
            .unwrap_or_else(|| std::cmp::max(MIN_RESERVED_OUTPUT, context_window / 10));
        let blocking_threshold = context_window.saturating_sub(reserved_output);

        if estimated_tokens > blocking_threshold {
            BlockingLimitDecision::Block {
                estimated_tokens,
                context_window,
            }
        } else {
            BlockingLimitDecision::Proceed
        }
    }

    /// Drive recovery for a stream that ended with a [`WithheldReason`].
    /// Consumes `assistant_msg` because every branch either pushes it
    /// to history or discards it deliberately (escalate retries
    /// produce a fresh response).
    ///
    /// `runtime` is borrowed read-only — the dispatcher only inspects
    /// [`ModelRuntime::current_client`]'s `ModelInfo` to derive the
    /// model-specific output-token ceiling. It does not advance the
    /// fallback chain or finalize probes; that lives in commit 4's
    /// `engine_fallback_retry`.
    pub(crate) async fn run_post_stream_recovery(
        &self,
        withheld: WithheldReason,
        assistant_msg: Message,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        turn_state: &mut LoopTurnState,
        active_snapshot: &ModelRuntimeSnapshot,
    ) -> RecoveryDisposition {
        let disposition = match withheld {
            WithheldReason::PromptTooLong => {
                self.recover_prompt_too_long(assistant_msg, history, event_tx, turn_state)
                    .await
            }
            WithheldReason::MaxOutputTokens => {
                self.recover_max_output_tokens(
                    assistant_msg,
                    history,
                    event_tx,
                    turn_state,
                    active_snapshot,
                )
                .await
            }
        };

        // **Finding R2** — reset `stop_hook_active` on every `continue`
        // site EXCEPT the StopHookBlocking one. Without this reset, a
        // prior Stop-hook block followed by a recovery continue would
        // re-fire stop hooks with `stop_hook_active: true` even though
        // the intervening recovery cleared that incident. `TerminateExhausted`
        // is unaffected — the caller falls through to the no-tool-calls
        // terminal where the C3 guard reads the flag once more, then the
        // loop exits.
        if matches!(disposition, RecoveryDisposition::Continue(_)) {
            turn_state.stop_hook_active = false;
        }
        disposition
    }

    /// Reactive compaction path. Pushes the partial assistant message
    /// so post-compact history retains the truncated content the model
    /// produced before hitting the wall, then delegates to the shared
    /// [`Self::handle_context_overflow`] (lives in `engine_finalize_turn`).
    ///
    /// **Finding R1** — when [`Self::handle_context_overflow`] returns
    /// [`ContextOverflowOutcome::Exhausted`] (circuit-breaker tripped or
    /// no tokens freed), this method pushes the synthetic api_error
    /// message tagged `prompt_too_long` and returns
    /// [`RecoveryDisposition::TerminateExhausted`]. Caller falls through
    /// to the no-tool-calls terminal where the C3 guard detects the
    /// api_error trailer, fires StopFailure hooks, and the engine emits
    /// `stop_reason = "prompt_too_long"` (derived from
    /// `ApiError.error_type`).
    async fn recover_prompt_too_long(
        &self,
        assistant_msg: Message,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        turn_state: &mut LoopTurnState,
    ) -> RecoveryDisposition {
        crate::history_sync::history_push_and_emit(history, assistant_msg, event_tx).await;
        match self
            .handle_context_overflow(history, event_tx, &mut turn_state.budget, "finish_reason")
            .await
        {
            ContextOverflowOutcome::Compacted(transition) => {
                RecoveryDisposition::Continue(transition)
            }
            ContextOverflowOutcome::Exhausted => {
                warn!("prompt-too-long recovery exhausted at finish-reason site; surfacing");
                crate::history_sync::history_push_and_emit(
                    history,
                    crate::helpers::build_abnormal_stop_api_error_message(
                        StopReason::ContextWindowExceeded,
                        /*effective_max_tokens*/ None,
                    ),
                    event_tx,
                )
                .await;
                RecoveryDisposition::TerminateExhausted
            }
        }
    }

    /// Two-phase output-budget recovery:
    ///
    /// 1. **Escalate** (opt-in per model): when
    ///    [`coco_config::ModelInfo::max_output_tokens_escalate`] is
    ///    `Some(N)` AND `N > max_output_tokens`, retry the same prompt
    ///    once with `N` as the per-call cap. Driven by setting
    ///    `turn_state.transition = MaxOutputTokensEscalate`; the API
    ///    call's `max_tokens` parameter resolves to the escalate
    ///    ceiling via [`effective_max_tokens`]. Does **not** push the
    ///    partial assistant message — the retry's larger budget
    ///    produces a complete response that supersedes it. Skipped
    ///    when the previous turn was already the escalate retry
    ///    (transition match) or when the model didn't opt in. To
    ///    disable escalation entirely for a model, leave
    ///    `max_output_tokens_escalate` unset in `~/.coco/models.json`;
    ///    to cap output to a fixed value, edit `max_output_tokens` for
    ///    that model — this is the multi-LLM-friendly single source
    ///    of truth, **not** a global env override.
    /// 2. **Resume**: inject a "pick up mid-thought" meta message,
    ///    push the partial response so the resume turn sees it. Runs
    ///    at most [`MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`] times.
    /// 3. **Exhausted**: push the partial response + the synthetic
    ///    api_error message so transcripts carry the truncation
    ///    marker; caller falls through to the no-tool-calls terminal.
    ///
    /// Phase-1 is **opt-in per model** to fit the multi-LLM
    /// architecture. Putting the ceiling in `ModelInfo` makes each
    /// model self-describe what it supports — and unset means
    /// "skip the escalate phase" rather than "guess a value that
    /// might break this provider."
    ///
    /// `active_client` is the **post-plan-swap** client that actually
    /// served the failing turn — the engine.rs caller computes it
    /// once (after `plan_swap_candidate` selection) and threads it
    /// here. Reading it instead of `runtime.current_client()` keeps
    /// the recovery decision aligned with what the next iteration's
    /// retry will hit. Otherwise plan-mode could fire Phase-1 against
    /// the Main role's escalate ceiling while the actual retry runs
    /// through the Plan role's (smaller, no-escalate) `ModelInfo` —
    /// a silent no-op.
    async fn recover_max_output_tokens(
        &self,
        assistant_msg: Message,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        turn_state: &mut LoopTurnState,
        active_snapshot: &ModelRuntimeSnapshot,
    ) -> RecoveryDisposition {
        // Phase 1: opt-in escalate. Gate on (a) the active model
        // declares an escalate ceiling > baseline, AND (b) this turn
        // is not itself the escalate retry (else we'd loop forever).
        // The transition-based gate is per-turn-only — no stateful
        // slot needed (which is why `LoopTurnState.max_tokens_override`
        // doesn't exist; the previous design's per-turn slot was the
        // root cause of three missing-reset bugs).
        //
        // No env-var gate by design: a global `COCO_MAX_OUTPUT_TOKENS`
        // would be a single-Anthropic-defaults regression in a
        // multi-LLM SDK. The per-model `ModelInfo.max_output_tokens` /
        // `max_output_tokens_escalate` pair IS the cap surface — to
        // pin output, edit `~/.coco/models.json`.
        let already_escalated = matches!(
            turn_state.transition,
            Some(ContinueReason::MaxOutputTokensEscalate)
        );
        let model_info = active_snapshot.model_info.as_ref();
        let baseline = model_info.map(|info| i64::from(info.max_output_tokens));
        let escalate_ceiling = model_info
            .and_then(|info| info.max_output_tokens_escalate)
            .map(i64::from);
        let phase1_available = match (baseline, escalate_ceiling) {
            (Some(b), Some(e)) => e > b,
            _ => false,
        };

        if phase1_available && !already_escalated {
            warn!(
                escalated_to = escalate_ceiling,
                baseline = baseline,
                provider = active_snapshot.provider,
                model_id = active_snapshot.model_id,
                "max_tokens hit, escalating to ModelInfo.max_output_tokens_escalate ceiling",
            );
            // No pushes — the retry produces a fresh assistant message
            // whose complete content replaces this truncated one. The
            // escalate value flows through `effective_max_tokens` next
            // iteration, driven by the transition match.
            return RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensEscalate);
        }

        // Phase 2: inject the resume-nudge meta message.
        if turn_state.max_tokens_recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT {
            turn_state.max_tokens_recovery_count += 1;
            warn!(
                attempt = turn_state.max_tokens_recovery_count,
                "max_tokens hit after escalation, injecting resume nudge",
            );
            crate::history_sync::history_push_and_emit(history, assistant_msg, event_tx).await;
            crate::history_sync::history_push_and_emit(
                history,
                coco_messages::create_meta_message(
                    "Output token limit hit. Resume directly — no apology, no recap of \
                     what you were doing. Pick up mid-thought if that is where the cut \
                     happened. Break remaining work into smaller pieces.",
                ),
                event_tx,
            )
            .await;
            return RecoveryDisposition::Continue(ContinueReason::MaxOutputTokensRecovery {
                attempt: turn_state.max_tokens_recovery_count,
            });
        }

        // Phase 3: exhausted. Surface both the partial response and
        // the synthetic api_error marker; caller falls through.
        warn!(
            attempts = turn_state.max_tokens_recovery_count,
            limit = MAX_OUTPUT_TOKENS_RECOVERY_LIMIT,
            "max_tokens recovery exhausted",
        );
        // The synthetic api_error reports whichever cap actually
        // clamped the last failed call: escalate ceiling (when this
        // turn was the escalate retry) or the model's baseline
        // `max_output_tokens`. Both come from the active `ModelInfo`.
        let effective_max = if already_escalated {
            escalate_ceiling
        } else {
            baseline
        };
        crate::history_sync::history_push_and_emit(history, assistant_msg, event_tx).await;
        crate::history_sync::history_push_and_emit(
            history,
            crate::helpers::build_abnormal_stop_api_error_message(
                StopReason::MaxTokens,
                effective_max,
            ),
            event_tx,
        )
        .await;
        RecoveryDisposition::TerminateExhausted
    }

    /// Record an observed capacity error (429 / 529) against
    /// [`coco_types::ToolAppState::rate_limits`] so post-turn forks
    /// (prompt-suggestion, …) see the throttle. Shared by the
    /// stream-open and mid-stream error paths so both observation
    /// points stay in lock-step (Finding **A1**).
    async fn record_capacity_observation(
        &self,
        active_snapshot: &ModelRuntimeSnapshot,
        kind: CapacityKind,
    ) {
        let Some(app_state) = self.app_state.as_ref() else {
            return;
        };
        crate::engine_helpers::record_rate_limit_observation(
            app_state,
            &active_snapshot.provider,
            active_snapshot.provider_api,
            kind.retry_after_ms,
        )
        .await;
    }

    /// Open the per-turn LLM stream and return the runtime call token
    /// that must be fed back after the stream is consumed.
    ///
    /// Returns `Ok(receiver)` for the success path or
    /// `Err(StreamErrorOutcome)` for both recoverable + unrecoverable
    /// stream-open failures. Caller turns the `Err` into
    /// `continue` / `return Err(_)` at the outer-loop site.
    ///
    /// Extraction motivated by R6 cleanup — the main loop's Ok/Err
    /// match block (74 LoC) collapses to a 5-LoC three-arm match here
    /// because the housekeeping naturally lives alongside the recovery
    /// dispatcher that shares state with it. `services` and
    /// `turn_state` are `&mut` for the same reason
    /// `handle_stream_open_error` is.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn open_turn_stream(
        &self,
        active_snapshot: &ModelRuntimeSnapshot,
        params: &coco_inference::QueryParams,
        services: &mut LoopServices,
        turn_state: &mut LoopTurnState,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        turn_id: &str,
    ) -> Result<OpenedTurnStream, StreamErrorOutcome> {
        match self
            .model_runtimes
            .open_stream_for_runtime(
                services.runtime.clone(),
                services.runtime_source.clone(),
                params,
            )
            .await
        {
            coco_inference::ModelStreamOpenOutcome::Opened {
                rx,
                token,
                snapshot,
                ..
            } => {
                tracing::debug!(
                    turn = turn_state.turn,
                    turn_id = %turn_id,
                    provider = snapshot.provider,
                    model_id = snapshot.model_id,
                    "LLM stream opened"
                );
                Ok(OpenedTurnStream {
                    rx,
                    token,
                    snapshot: *snapshot,
                })
            }
            coco_inference::ModelStreamOpenOutcome::Retry { events } => {
                turn_state.count_next_iteration_as_turn = false;
                self.record_capacity_observation(
                    active_snapshot,
                    CapacityKind {
                        retry_after_ms: None,
                    },
                )
                .await;
                for event in events {
                    if let coco_inference::ModelRuntimeEvent::FallbackSwitched {
                        from_model_id,
                        to_model_id,
                        ..
                    } = event
                    {
                        self.post_advance_side_effects(&active_snapshot.provider, services)
                            .await;
                        emit_model_fallback_notice(
                            event_tx,
                            &from_model_id,
                            &to_model_id,
                            &self.config.session_id,
                            crate::model_runtime::ModelFallbackReason::CapacityDegrade {
                                consecutive_errors: 1,
                            },
                        )
                        .await;
                    }
                }
                Err(StreamErrorOutcome::Continue)
            }
            coco_inference::ModelStreamOpenOutcome::Failed { error, events } => Err(self
                .handle_stream_open_error(
                    error,
                    active_snapshot,
                    events,
                    services,
                    turn_state,
                    history,
                    event_tx,
                )
                .await),
        }
    }

    /// Drive recovery for a stream-open error (`active_client::query_stream`
    /// returned `Err`). Mirrors the body of [`Self::handle_stream_error`]
    /// but for the pre-stream-open site — no `streaming_handle` to
    /// discard, no `turn_id` for tool-completion telemetry.
    ///
    /// Behavior parity with the mid-stream sibling:
    /// 1. probe-in-flight ⇒ revert + `Continue` (probes must never
    ///    surface as user-visible failures).
    /// 2. typed `ContextWindowExceeded` ⇒ reactive compact + `Continue`.
    /// 3. capacity (typed `Overloaded` / `RateLimited`, OR string
    ///    fallback via [`crate::engine_helpers::is_capacity_error_message`]
    ///    for the `ProviderError`-wrapped path the vercel-ai retry layer
    ///    occasionally produces): record rate-limit observation, then
    ///    consume the runtime's fallback switch or final failure.
    /// 4. anything else ⇒ `Bail` with a `ProviderError`-classified
    ///    [`coco_error::PlainError`].
    ///
    /// The caller passes `active_client` so the rate-limit observation
    /// keys against the model that *actually* served the failed request
    /// (plan-mode swap may have routed this turn through a non-default
    /// client). Streak / advance still operate on `services.runtime`
    /// because the runtime is the canonical fallback state machine —
    /// when a plan-swap call fails we advance the main runtime slot, not
    /// the plan slot.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_stream_open_error(
        &self,
        e: coco_inference::InferenceError,
        active_snapshot: &ModelRuntimeSnapshot,
        runtime_events: Vec<coco_inference::ModelRuntimeEvent>,
        services: &mut LoopServices,
        turn_state: &mut LoopTurnState,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
    ) -> StreamErrorOutcome {
        let err_msg = e.to_string();
        // Typed enum match only — `coco-inference`'s provider seam maps
        // every multi-provider context-overflow signal into this
        // variant. String fallback removed (C1 fix).
        if matches!(
            &e,
            coco_inference::InferenceError::ContextWindowExceeded { .. }
        ) {
            match self
                .handle_context_overflow(history, event_tx, &mut turn_state.budget, "stream_open")
                .await
            {
                ContextOverflowOutcome::Compacted(transition) => {
                    turn_state.transition = Some(transition);
                    return StreamErrorOutcome::Continue;
                }
                ContextOverflowOutcome::Exhausted => {
                    // Finding **R1** — push synthetic api_error so the
                    // session transcript carries the prompt_too_long
                    // marker, then bail. No partial response existed
                    // (stream never opened), so the engine's Err path
                    // is the right exit. The PlainError is tagged
                    // `ContextWindowExceeded` for status-code routing.
                    crate::history_sync::history_push_and_emit(
                        history,
                        crate::helpers::build_abnormal_stop_api_error_message(
                            StopReason::ContextWindowExceeded,
                            /*effective_max_tokens*/ None,
                        ),
                        event_tx,
                    )
                    .await;
                    return StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
                        format!(
                            "LLM stream open failed: context window exceeded \
                             and reactive compaction could not recover ({e})"
                        ),
                        coco_error::StatusCode::ContextWindowExceeded,
                    )));
                }
            }
        }
        // Capacity check uses the typed enum first; the string fallback
        // covers the vercel-ai retry layer's occasional non-typed
        // surfacing as a generic `ProviderError`. Tightening that seam
        // is a vercel-ai-* concern tracked separately.
        let capacity_kind = capacity_kind_from(Some(&e)).or_else(|| {
            if crate::engine_helpers::is_capacity_error_message(&err_msg) {
                Some(CapacityKind {
                    retry_after_ms: None,
                })
            } else {
                None
            }
        });
        if let Some(kind) = capacity_kind {
            self.record_capacity_observation(active_snapshot, kind)
                .await;
            let original_provider = active_snapshot.provider.clone();
            let events = runtime_events;
            if events.is_empty() {
                warn!(
                    active = services.current_model_id(),
                    "capacity error recorded by model runtime; surfacing error",
                );
                return StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
                    format!("LLM stream open failed: {e}"),
                    coco_error::StatusCode::ProviderError,
                )));
            }
            for event in events {
                if let coco_inference::ModelRuntimeEvent::FallbackSwitched {
                    from_model_id,
                    to_model_id,
                    ..
                } = event
                {
                    self.post_advance_side_effects(&original_provider, services)
                        .await;
                    emit_model_fallback_notice(
                        event_tx,
                        &from_model_id,
                        &to_model_id,
                        &self.config.session_id,
                        crate::model_runtime::ModelFallbackReason::CapacityDegrade {
                            consecutive_errors: 1,
                        },
                    )
                    .await;
                    turn_state.count_next_iteration_as_turn = false;
                    return StreamErrorOutcome::Continue;
                }
            }
        }
        StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
            format!("LLM stream open failed: {e}"),
            coco_error::StatusCode::ProviderError,
        )))
    }

    /// Drive recovery for a mid-stream `StreamEvent::Error`. Mirrors
    /// the inline branch the engine previously embedded in
    /// `run_session_loop` directly after `consume_stream` returned —
    /// classifies the error string into a typed
    /// [`coco_inference::InferenceError`], routes to the appropriate
    /// recovery (reactive compact / runtime fallback), and either signals
    /// [`StreamErrorOutcome::Continue`] for the outer loop or
    /// [`StreamErrorOutcome::Bail`] with a typed `BoxedError`.
    ///
    /// Side effects bundled here (matching the previous inline form):
    /// * runtime-owned model communication feedback
    /// * `turn_state.budget` mutation via `handle_context_overflow`
    /// * runtime-owned capacity feedback
    /// * runtime-owned fallback switch
    /// * `cache_break_reset` via `post_advance_side_effects`
    /// * `emit_model_fallback_notice` for both Switched + Exhausted
    /// * `streaming_handle.discard()` with per-outcome
    ///   `ToolUseCompleted{is_error: true}` emission on the bail path
    ///
    /// Generic over `StreamingHandle<F, Fut>` so the engine forwards
    /// `&mut streaming_handle` without boxing. Bounds copied from
    /// `core/tool-runtime/src/executor_streaming.rs:90-94`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_stream_error<F, Fut>(
        &self,
        err_msg: String,
        had_output: bool,
        services: &mut LoopServices,
        token: &coco_inference::ModelCallHandle,
        turn_state: &mut LoopTurnState,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<crate::CoreEvent>>,
        streaming_handle: &mut Option<StreamingHandle<F, Fut>>,
        turn_id: &str,
    ) -> StreamErrorOutcome
    where
        F: Fn(PreparedToolCall, RunOneRuntime) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = UnstampedToolCallOutcome> + Send + 'static,
    {
        // Classify the mid-stream error via the typed `InferenceError`
        // taxonomy. The provider-specific keyword sniffing lives in
        // `coco_inference::errors` — the engine layer matches on the
        // enum variant per the multi-provider boundary rule in
        // `CLAUDE.md`.
        let classified = coco_inference::InferenceError::classify_stream_message(&err_msg);
        if matches!(
            classified,
            Some(coco_inference::InferenceError::ContextWindowExceeded { .. })
        ) {
            match self
                .handle_context_overflow(history, event_tx, &mut turn_state.budget, "mid_stream")
                .await
            {
                ContextOverflowOutcome::Compacted(transition) => {
                    turn_state.transition = Some(transition);
                    return StreamErrorOutcome::Continue;
                }
                ContextOverflowOutcome::Exhausted => {
                    // Finding **R1** — same shape as the stream-open
                    // sibling: push synthetic api_error + bail. The
                    // mid-stream partial wasn't committed (it lives in
                    // the snapshot but the stream errored before
                    // `assistant_msg` construction), so the engine's
                    // Err path is again the right exit.
                    crate::history_sync::history_push_and_emit(
                        history,
                        crate::helpers::build_abnormal_stop_api_error_message(
                            StopReason::ContextWindowExceeded,
                            /*effective_max_tokens*/ None,
                        ),
                        event_tx,
                    )
                    .await;
                    return StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
                        format!(
                            "LLM stream failed mid-stream: context window exceeded \
                             and reactive compaction could not recover ({err_msg})"
                        ),
                        coco_error::StatusCode::ContextWindowExceeded,
                    )));
                }
            }
        }
        if let Some(capacity) = capacity_kind_from(classified.as_ref()) {
            // Finding A1 (mid-stream parity with stream-open):
            // mid-stream 429 / 529 must also propagate to
            // `ToolAppState.rate_limits` so post-turn forks
            // (prompt-suggestion) see the throttle.
            let snapshot = services.snapshot();
            self.record_capacity_observation(&snapshot, capacity).await;
            let original_provider = snapshot.provider;
            let feedback = self
                .model_runtimes
                .finish_call_for_retry(
                    token,
                    coco_inference::ModelCommunicationOutcome::Capacity {
                        retry_after_ms: capacity.retry_after_ms,
                    },
                )
                .await;
            let coco_inference::ModelRuntimeFeedbackOutcome::Retry { events } = feedback else {
                // Fallback chain is unavailable or exhausted. If nothing
                // was emitted to the user yet, this mid-stream capacity
                // error is equivalent to a handshake throttle — the
                // identical request can be re-issued in place without
                // duplicating visible output. Bounded + backoff (honoring
                // server retry-after) so a persistently-saturated single
                // model still surfaces the error, mirroring the in-place
                // retry the handshake path gets in
                // `client.rs::query_stream_with_config`.
                if !had_output
                    && turn_state.stream_capacity_retries < MAX_MIDSTREAM_CAPACITY_RETRIES
                {
                    turn_state.stream_capacity_retries += 1;
                    let delay = midstream_capacity_backoff(
                        capacity.retry_after_ms,
                        turn_state.stream_capacity_retries,
                    );
                    warn!(
                        active = services.current_model_id(),
                        attempt = turn_state.stream_capacity_retries,
                        delay_ms = delay.as_millis() as i64,
                        "capacity error mid-stream, no output emitted; retrying in place",
                    );
                    tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => {}
                        _ = tokio::time::sleep(delay) => {}
                    }
                    turn_state.count_next_iteration_as_turn = false;
                    return StreamErrorOutcome::Continue;
                }
                warn!(
                    active = services.current_model_id(),
                    "capacity error mid-stream recorded by model runtime; surfacing error",
                );
                return StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
                    format!("LLM stream failed: {err_msg}"),
                    coco_error::StatusCode::ProviderError,
                )));
            };
            for event in events {
                if let coco_inference::ModelRuntimeEvent::FallbackSwitched {
                    from_model_id,
                    to_model_id,
                    ..
                } = event
                {
                    self.post_advance_side_effects(&original_provider, services)
                        .await;
                    emit_model_fallback_notice(
                        event_tx,
                        &from_model_id,
                        &to_model_id,
                        &self.config.session_id,
                        crate::model_runtime::ModelFallbackReason::CapacityDegrade {
                            consecutive_errors: 1,
                        },
                    )
                    .await;
                    turn_state.count_next_iteration_as_turn = false;
                    return StreamErrorOutcome::Continue;
                }
            }
            turn_state.count_next_iteration_as_turn = false;
            return StreamErrorOutcome::Continue;
        }
        // Surface streaming-discard outcomes for telemetry before
        // bailing out. The assistant message hasn't committed yet on
        // this path, so committing tool_result rows to history would
        // violate I1; instead we emit `ToolUseCompleted{is_error}` per
        // discarded plan and warn-log a summary, then drop them.
        // Without this drain `JoinSet::drop` aborts inflight safe
        // tools silently — operators lose visibility into how much
        // real work the stream error invalidated.
        if let Some(handle) = streaming_handle.take() {
            let discarded = handle.discard().await;
            if !discarded.is_empty() {
                let count = discarded.len() as i64;
                for outcome in discarded {
                    let tool_use_id = outcome.tool_use_id.clone();
                    let tool_id = outcome.tool_id.clone();
                    let text = extract_streaming_result_text(&outcome.ordered_messages);
                    let _ = crate::emit::emit_stream(
                        event_tx,
                        crate::AgentStreamEvent::ToolUseCompleted {
                            call_id: tool_use_id,
                            name: tool_id.to_string(),
                            output: text,
                            is_error: true,
                        },
                    )
                    .await;
                }
                warn!(
                    turn = turn_state.turn,
                    turn_id = %turn_id,
                    discarded_count = count,
                    error = %err_msg,
                    "discarded streaming tool outcomes after mid-stream error",
                );
            }
        }
        self.model_runtimes
            .finish_call(token, coco_inference::ModelCommunicationOutcome::Failure);
        StreamErrorOutcome::Bail(Box::new(coco_error::PlainError::new(
            format!("LLM stream failed: {err_msg}"),
            coco_error::StatusCode::ProviderError,
        )))
    }
}
