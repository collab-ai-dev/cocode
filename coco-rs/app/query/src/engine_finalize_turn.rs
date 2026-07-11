//! Per-turn tail of [`QueryEngine::run_session_loop`] + reactive recovery.
//!
//! Owns:
//! - [`QueryEngine::finalize_turn_post_tools`] — the tail-of-turn ladder that
//!   drains the command queue + inbox, runs the auto-compact ladder
//!   (time-based microcompact → file-stub cleanup → SM extraction →
//!   threshold microcompact → SM-first / full LLM), and emits
//!   `TurnCompleted`.
//! - [`QueryEngine::do_reactive_compact`] — `prompt_too_long` recovery.
//!   Capability-split between Anthropic's server-side `context_management`
//!   (cache-preserving) and the client-side `api_microcompact` +
//!   `peel_head_for_ptl_retry` fallback.
//!
//! Extracted from `engine.rs` to keep the multi-turn loop file focused on
//! orchestration. The full LLM / SM / manual compact paths live in
//! `crate::engine_compaction`.

#[path = "engine_finalize_tail.rs"]
mod tail;
use tail::*;

use std::sync::Arc;

use tracing::info;
use tracing::warn;

use coco_messages::MessageHistory;
use coco_types::TokenUsage;

use crate::ContinueReason;
use crate::CoreEvent;
use crate::ServerNotification;
use crate::budget::BudgetTracker;
use crate::command_queue::QueuePriority;
use crate::emit::emit_protocol;
use crate::engine::QueryEngine;
use crate::helpers::drain_command_queue_into_history;

/// Whether the session loop will continue with another LLM round or
/// return immediately after this finalize call.
///
/// Gates `TurnEnded(Completed)` wire emission inside
/// [`QueryEngine::finalize_turn_post_tools`] / [`QueryEngine::finalize_successful_turn_tail`].
/// Per-turn bookkeeping (queue drain, compaction, transcript flush,
/// reasoning-metadata side-cache) runs in both modes.
///
/// `stop_reason` is **not** part of this enum: control-flow state
/// must not launder a model finish reason. Callers pass
/// `Option<StopReason>` separately to [`Self::emit_turn_ended_completed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnContinuation {
    /// Another LLM round will fire for the same user-prompt cycle.
    /// Do *not* emit terminal `TurnEnded` — the SDK iterator and TUI
    /// state machine treat that event as "user-prompt cycle done".
    Continuing,
    /// The session loop is about to return: this is the last LLM round
    /// for the current user prompt. Emit `TurnEnded(Completed)`.
    Terminal,
}

impl TurnContinuation {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, TurnContinuation::Terminal)
    }
}

impl QueryEngine {
    /// Unified handler for "input + output won't fit in the model's context
    /// window." Three distinct signals route here:
    ///
    /// 1. HTTP 400 [`coco_inference::InferenceError::ContextWindowExceeded`] —
    ///    provider rejected the request outright (OpenAI / Google / ByteDance
    ///    `context_length_exceeded`, defensive `prompt_too_long` body match).
    /// 2. Mid-stream error string `prompt_too_long` / `context_length` — same
    ///    signal arriving after `message_start` but before the response
    ///    completes.
    /// 3. Anthropic [`coco_messages::StopReason::ContextWindowExceeded`]
    ///    finish reason (extended-context beta only) — request streamed
    ///    cleanly to a finish event whose stop_reason reports window
    ///    exhaustion.
    ///
    /// Always attempts reactive compaction; never escalates
    /// `max_output_tokens`. Raising the output budget cannot help when the
    /// *input* already exceeds the window — it only delays the next failure
    /// by another round-trip and (on the Anthropic finish-reason path) makes
    /// the next request trip the HTTP-400 sibling. Compaction shrinks the
    /// actual culprit. `do_reactive_compact` carries its own 3-failure
    /// circuit breaker, so repeated calls cannot spin.
    ///
    /// `site` is purely a tracing field for distinguishing the three call
    /// sites in logs.
    ///
    /// Returns [`crate::engine_recovery::ContextOverflowOutcome::Compacted`]
    /// with the retry transition when compaction freed any tokens, and
    /// [`crate::engine_recovery::ContextOverflowOutcome::Exhausted`] when
    /// compaction was a no-op (circuit-breaker tripped or no progress).
    /// Caller propagates `Exhausted` to a terminal exit (push synthetic
    /// api_error → `TerminateExhausted` from recovery, or `Bail` from
    /// the stream-open / mid-stream sites). When reactive compaction returns
    /// null, surface withheld lastMessage + `executeStopFailureHooks` +
    /// return `'prompt_too_long'`. Without this signal, the loop would spin
    /// until `BudgetTracker::Stop` (Finding **R1**).
    pub(crate) async fn handle_context_overflow(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        budget: &mut BudgetTracker,
        site: &'static str,
    ) -> crate::engine_recovery::ContextOverflowOutcome {
        warn!(
            site,
            "context window exceeded, attempting reactive compaction"
        );
        let made_progress = self.do_reactive_compact(history, event_tx).await;
        budget.reset_continuations();
        if made_progress {
            crate::engine_recovery::ContextOverflowOutcome::Compacted(
                ContinueReason::ReactiveCompactRetry,
            )
        } else {
            warn!(
                site,
                "reactive compaction made no progress (circuit-breaker tripped \
                 or no tokens freed); surfacing as terminal recovery exhaustion",
            );
            crate::engine_recovery::ContextOverflowOutcome::Exhausted
        }
    }

    /// Shrink `history` with a reactive microcompact and emit the paired
    /// `CompactionStarted` → `ContextCompacted` notifications. Shared by every
    /// context-window-exceeded recovery site (stream-open 400, mid-stream
    /// error, and Anthropic `model_context_window_exceeded` finish reason) —
    /// keeps the three paths bit-identical.
    ///
    /// Returns `true` when compaction freed at least one token (the success
    /// arm of `record_success`/`record_failure` bookkeeping below) and
    /// `false` on circuit-breaker skip or no-progress runs. Caller uses
    /// this signal to escalate "compaction can't help" to a terminal exit
    /// (Finding **R1**).
    #[tracing::instrument(
        skip_all,
        name = "compaction",
        fields(
            trigger = "reactive",
            session_id = %self.config.session_id,
            history_len = history.len(),
        ),
    )]
    pub(crate) async fn do_reactive_compact(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) -> bool {
        // Circuit-breaker check: if we've already failed 3× in a row,
        // don't keep wasting API calls.
        {
            let state = self.reactive_state.lock().await;
            if !state.should_attempt_reactive_compact() {
                warn!(
                    failures = state.failure_count(),
                    "reactive compact circuit-breaker tripped; skipping"
                );
                return false;
            }
        }

        let pre_tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
        let pre_count = history.len() as i32;
        let drop_target = coco_compact::reactive::calculate_drop_target(
            pre_tokens,
            &coco_compact::ReactiveCompactConfig {
                context_window: self.resolved_context_window(),
                max_output_tokens: self.resolved_max_output_tokens(),
                ..Default::default()
            },
            &self.config.compact.auto,
        );
        let _ = emit_protocol(event_tx, ServerNotification::CompactionStarted).await;

        // Step 0: if staged-collapse is active, try draining staged
        // ranges into commits before falling back to head-truncation.
        // Drained commits don't strip
        // messages here — they only mark them as committed; the next
        // `apply_collapses_if_needed` (run before each prompt build)
        // performs the actual splice. Until that pass is wired, this
        // path emits a phase event so TUI can show the recovery and
        // proceeds to the standard reactive microcompact below.
        let drained: Vec<coco_compact::StagedCommitEntry> =
            if let Some(ledger) = &self.staged_ledger {
                let mut g = ledger.lock().await;
                g.drain_overflow(self.staged_session_id.clone(), |_| uuid::Uuid::new_v4())
            } else {
                Vec::new()
            };
        if !drained.is_empty() {
            info!(
                drained = drained.len(),
                "PTL recovery: drained staged collapses into commits"
            );
            // Persist each drained commit so resume can replay them.
            if let (Some(store), Some(sid)) = (&self.transcript_store, &self.transcript_session_id)
            {
                for entry in &drained {
                    if let Ok(payload) = serde_json::to_value(entry)
                        && let Err(e) = store.append_marble_origami_commit(sid.as_str(), payload)
                    {
                        warn!("failed to persist marble-origami-commit: {e}");
                    }
                }
                // Persist the (now-empty) snapshot so resume sees the
                // armed=false state. Last-wins semantics make this safe.
                if let Some(ledger) = &self.staged_ledger {
                    let g = ledger.lock().await;
                    if let Some(snap) = g.snapshot.as_ref()
                        && let Ok(payload) = serde_json::to_value(snap)
                        && let Err(e) = store.append_marble_origami_snapshot(sid.as_str(), payload)
                    {
                        warn!("failed to persist marble-origami-snapshot: {e}");
                    }
                }
            }
        }

        // Provider capability split. On Anthropic (server-side edits) we
        // attach a one-shot `context_management` payload to the next
        // QueryParams build instead of mutating messages locally — the API
        // clears tool results in place and the prompt cache stays intact. On
        // other providers, fall back to the client-side mutation path
        // (cache-invalidating but universal).
        //
        // Loop guard: the server-side branch frees no LOCAL tokens (it queues
        // an edit the retry will send). If the PREVIOUS attempt already queued
        // a server-side edit and we are back here, that cache-preserving retry
        // did NOT resolve the overflow — fall back to client-side truncation
        // (which frees real tokens) so we don't queue-and-retry forever.
        let server_side_supported = self
            .runtime_snapshot()
            .is_some_and(|snapshot| snapshot.supports_server_side_context_edits);
        let prior_server_side_unresolved = {
            let mut state = self.reactive_state.lock().await;
            state.take_pending_server_side()
        };
        let use_server_side = server_side_supported && !prior_server_side_unresolved;
        if use_server_side {
            // Build aggressive ApiContextOptions from current state.
            // `trigger_threshold = pre_tokens` ensures the server applies
            // clearing for the current oversized prompt; `keep_target`
            // aims for `pre_tokens - drop_target` so the server frees at
            // least `drop_target` worth.
            let opts = coco_compact::ApiContextOptions {
                has_thinking: self.config.thinking_level.is_some(),
                is_redact_thinking_active: false,
                clear_all_thinking: true,
                clear_tool_results: true,
                clear_tool_uses: true,
                trigger_threshold: pre_tokens.max(1),
                keep_target: (pre_tokens - drop_target).max(1),
            };
            let strategies = coco_compact::get_api_context_management(&opts);
            if let Some(payload) = coco_compact::encode_anthropic_context_management(&strategies) {
                let mut pending = self.pending_reactive_context_management.lock().await;
                *pending = Some(payload);
                info!(
                    drop_target,
                    "queued reactive context_management for next API call"
                );
            }
            // Server clears in place — no local mutation. The next API
            // call sends the original (oversized) prompt + the payload;
            // Anthropic strips and bills accordingly.
        } else {
            history.with_owned_messages(|msgs| {
                coco_compact::reactive::api_microcompact(msgs, drop_target);
            });
            let post_micro_tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
            let freed = (pre_tokens - post_micro_tokens).max(0);

            // Escalate when api_microcompact couldn't free enough — most
            // likely all old tool results are already cleared. Peel oldest
            // API-round groups until we've freed `drop_target` tokens.
            // Head-truncation falls back here when the in-place tool-result
            // clear can't recover budget.
            if freed < drop_target
                && let Some(survivors) =
                    coco_compact::peel_head_for_ptl_retry(history.as_slice(), drop_target - freed)
            {
                // I-1 (Authority): reactive head-trim drops oldest
                // messages from history. Pair the swap with truncate
                // + appended-burst so TUI/SDK observers see the new
                // state.
                crate::history_sync::history_replace_and_emit(
                    history,
                    survivors,
                    event_tx,
                    coco_types::HistoryReplaceReason::Trim,
                )
                .await;
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let post_tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
        let actually_freed = (pre_tokens - post_tokens).max(0);

        // Progress signal for `handle_context_overflow`. The server-side
        // (Anthropic) branch frees no LOCAL tokens — it queues a
        // `context_management` edit the next request will carry, and the API
        // clears in place. So "queued" IS progress: returning false here would
        // make the very first context-overflow turn terminate as Exhausted
        // before the cache-preserving retry ever runs (the original bug). The
        // `pending_server_side` flag (taken at the top) bounds this — a
        // re-entry after a server-side queue falls back to the client-side
        // branch below, which counts progress by tokens actually dropped.
        let made_progress = {
            let mut state = self.reactive_state.lock().await;
            if use_server_side {
                state.set_pending_server_side();
                state.record_success(now_ms);
                true
            } else if actually_freed > 0 {
                state.record_success(now_ms);
                true
            } else {
                state.record_failure(now_ms);
                false
            }
        };

        let removed = (pre_count - history.len() as i32).max(0);
        let _ = emit_protocol(
            event_tx,
            ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                removed_messages: removed,
                summary_tokens: 0,
                trigger: coco_types::CompactTrigger::Reactive,
                pre_tokens: Some(pre_tokens),
                post_tokens: Some(post_tokens),
            }),
        )
        .await;

        // Reactive recovery shares the post-compact-cleanup path with
        // full / SM compaction.
        // We build a synthetic CompactResult — observers in
        // `app/query/src/observers.rs` only inspect `trigger` /
        // `is_main_agent`, not summary content, so empty fields are fine —
        // `messages_to_keep: Vec::new()` saves an N-message deep clone that
        // would have been thrown away after the observer dispatch.
        let is_main_agent = self.config.agent_id.is_none();
        let synth = coco_compact::CompactResult {
            boundary_marker: coco_messages::create_compact_boundary_message(
                pre_tokens,
                post_tokens,
            ),
            raw_summary: None,
            summary_messages: Vec::new(),
            attachments: Vec::new(),
            messages_to_keep: Vec::new(),
            hook_results: Vec::new(),
            user_display_message: None,
            pre_compact_tokens: pre_tokens,
            post_compact_tokens: post_tokens,
            true_post_compact_tokens: post_tokens,
            is_recompaction: false,
            trigger: coco_types::CompactTrigger::Reactive,
        };
        self.compaction_observers
            .notify_all(&synth, is_main_agent)
            .await;
        self.compaction_observers
            .notify_post_compact(history.as_slice())
            .await;

        // Reset the cache-break baseline. Reactive shares the
        // `repl_main_thread` tracking key with main loop, so use the same
        // source attribution as the API call site. After this, the next
        // response's lower cache_read tokens won't false-positive as a break.
        let qs = self.query_source_label();
        self.notify_model_compaction(qs).await;

        // The CLIENT-SIDE reactive compact rewrites history (peels oldest
        // API-round groups, drops attachments), so it must reset the memory
        // recall state AND clear the SM cache — same as full / SM-first /
        // partial compact. The SERVER-SIDE (Anthropic) branch only queues an
        // edit and leaves local history intact, so its recall/SM state is still
        // valid; resetting there would needlessly kill recall for the rest of
        // the session.
        if !use_server_side && let Some(rt) = &self.memory_runtime {
            rt.reset_recall_state();
            rt.session_memory.clear_after_compact().await;
        }

        // The next reminder build consumes the observed compact epoch.
        self.pending_just_compacted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Progress signal for `handle_context_overflow` / recovery
        // dispatcher (Finding **R1**). Client-side counts tokens freed;
        // server-side counts the queued edit as progress (see `made_progress`
        // above). Zero-progress runs surface as terminal exhaustion.
        made_progress
    }

    /// Finalize a turn after tools have executed: drain queued commands + inbox,
    /// auto-compact if over threshold, stamp reasoning metadata, and — when
    /// the session loop is about to return — emit `TurnCompleted`.
    ///
    /// `continuation` reflects what the session loop will do **next** (not what
    /// the LLM's `stop_reason` was): the loop may continue after a `ToolUse`
    /// stop, or terminate after a `ToolUse` if a tool called
    /// `prevent_continuation()` / the tool runner reported
    /// `continue_after_tools = false`. The wire-event invariant — exactly one
    /// `TurnCompleted` per user-prompt cycle — is keyed on "is the loop about
    /// to exit", not on `stop_reason`. Per-turn bookkeeping (queue drain,
    /// compaction, transcript flush, reasoning metadata) runs unconditionally;
    /// the protocol event is gated.
    ///
    /// `TurnCompleted` is wire-protocol-load-bearing for Rust consumers (no
    /// async-generator-return equivalent in NDJSON RPC) — the Python SDK
    /// iterator, the TUI state machine, and the SDK dispatcher's
    /// `StreamAccumulator` flush all key on it.
    // Crate-internal helper. `cycle_turn_id` + `stop_reason` are the
    // wire-emit gate; `usage` drives both the protocol payload and the
    // reasoning-metadata side-cache lookup. The per-round `turn_id`
    // string used to ride along for log correlation — it's now stamped
    // upstream in `run_session_loop`'s info span and dropped from the
    // signature here.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn finalize_turn_post_tools(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        usage: TokenUsage,
        continuation: TurnContinuation,
        cycle_turn_id: Option<coco_types::TurnId>,
        stop_reason: Option<coco_messages::StopReason>,
        // #3: whether a Sleep tool ran in the just-completed batch.
        // `Later`-priority items (background task-completion notifications)
        // drain only after a Sleep; else the boundary drain caps at `Next`.
        sleep_ran: bool,
    ) {
        // Periodic terminal-task eviction. Fires every turn regardless
        // of success / failure / cancellation outcome. Without a periodic
        // sweep `TaskManager`'s in-memory map grows monotonically over a
        // long session. The panel-grace gate is enforced inside
        // (`remove_completed` keeps `retain == true` or
        // `evict_after > now` tasks).
        if let Some(running) = self.running_tasks.as_ref() {
            let removed = running.remove_completed().await;
            if removed > 0 {
                tracing::trace!(
                    target: "coco_query::task_runtime",
                    removed,
                    "per-turn evicted terminal tasks past panel-grace"
                );
            }
        }

        // Tool-use-summary side-fork spawns **immediately** after
        // `query_tool_execution_end`, BEFORE any post-tool processing
        // (queue drain, microcompact, auto-compact, memory fan-out).
        // The spawn captures the just-executed batch
        // (last assistant + matching tool results) from `history`; any
        // later compaction would summarize history and lose the batch
        // we want to label.
        //
        // Gated on:
        //   * `Feature::ToolUseSummary` enabled (default off — UX polish
        //     that silently degrades on reasoning Fast models)
        //   * model runtime registry wired (Fast role configured)
        //   * `agent_id.is_none()` (subagent skip)
        //   * tool batch non-empty (handled inside the spawn helper)
        // Never blocks; failure modes degrade to `None`.
        self.spawn_tool_use_summary(history).await;

        // Post-compact turn counter: no-op when no compact has happened yet
        // (`last_compact_state == None`). Lock is brief; only held at turn
        // boundaries.
        if let Ok(mut guard) = self.last_compact_state.lock()
            && let Some(state) = guard.as_mut()
        {
            state.turn_counter = state.turn_counter.saturating_add(1);
        }
        {
            let mut state = self.auto_compact_state.lock().await;
            state.advance_turn();
        }

        // Drain command queue: all priorities land before the next API
        // call. Slash commands excluded (processed post-turn).
        // Agent-filtered.
        //
        // The queue carries every steering producer through one pipe:
        // human keyboard input (`QueueOrigin::Human`), coordinator
        // teammate messages (`QueueOrigin::Coordinator`), background
        // task completions (`QueueOrigin::TaskNotification`), and MCP
        // channel messages (`QueueOrigin::Channel`). Each item drains
        // into history as a `Message::Attachment` of kind
        // `QueuedCommand` with origin-specific framing prepended via
        // `wrap_command_text` wraps each item (human prompts,
        // coordinator messages) as `attachment.type === 'queued_command'`.
        // #3: `later`-priority items (background task notifications) drain
        // only when a Sleep tool ran this batch; otherwise cap at `next`.
        let drain_priority = if sleep_ran {
            QueuePriority::Later
        } else {
            QueuePriority::Next
        };
        drain_command_queue_into_history(
            &self.command_queue,
            history,
            event_tx,
            drain_priority,
            self.config.agent_id_str(),
        )
        .await;

        // Auto-compaction ladder (tail-of-turn):
        //  0. Time-based microcompact — fire on long inactivity gap so the
        //     next API call doesn't carry stale tool result content.
        //  1. Threshold micro_compact — keep last N compactable tool uses.
        //  2. Session-memory-first — replace LLM summary with pre-extracted
        //     memory when the post-SM count would still fit.
        //  3. Full LLM compact — fallback when SM declined or wasn't enabled.
        //
        // `should_auto_compact_guarded` reads the resolved
        // `AutoCompactConfig` (user toggle + env kill switches +
        // overrides folded in by `coco_config::CompactConfig::resolve`)
        // and adds the recursion guard. `Other` source = main thread /
        // SDK; subagent paths set their own source when wired through.
        let auto_cfg = &self.config.compact.auto;
        let micro_keep = self.config.compact.micro.keep_recent.max(0) as usize;

        // Step 0: time-based microcompact (gap > threshold && main thread).
        // Independent of token threshold — fires whenever the cache TTL has
        // likely expired, preventing stale tool results from poisoning the
        // next prompt cache.
        let tb_cfg = &self.config.compact.micro.time_based;
        if self.config.compact.micro.enabled && tb_cfg.enabled {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let last_ms = self
                .last_assistant_ms
                .load(std::sync::atomic::Ordering::Acquire);
            let last_opt = if last_ms > 0 { Some(last_ms) } else { None };
            if let Some(trigger) = coco_compact::evaluate_time_based_trigger(
                tb_cfg, now_ms, last_opt, /*is_main_thread*/ true,
            ) {
                let pre_tb_tokens = coco_messages::estimate_tokens_for_messages(history.as_slice());
                if let Some(res) = history.with_owned_messages(|msgs| {
                    coco_compact::time_based_microcompact(msgs, &trigger)
                }) {
                    info!(
                        cleared = res.messages_cleared,
                        gap_min = trigger.gap_minutes,
                        "time-based micro-compaction triggered",
                    );
                    let post_tb_tokens =
                        coco_messages::estimate_tokens_for_messages(history.as_slice());
                    // Reuse `Auto` for the boundary trigger taxonomy;
                    // the `TimeBased` variant remains for callers that still
                    // want the distinction in custom UIs.
                    let _ = emit_protocol(
                        event_tx,
                        ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                            removed_messages: res.messages_cleared,
                            summary_tokens: 0,
                            trigger: coco_types::CompactTrigger::Auto,
                            pre_tokens: Some(pre_tb_tokens),
                            post_tokens: Some(post_tb_tokens),
                        }),
                    )
                    .await;
                    // The next response's cache_read drop is from us, not a
                    // real break. Use the same query_source attribution as
                    // the main API call so they share the tracking key.
                    let qs = self.query_source_label();
                    self.notify_model_cache_deletion(qs).await;
                }
            }
        }

        // Step 0.5: file-unchanged stub cleanup. After many turns of
        // re-reading the same file, accumulated `[file unchanged]`
        // tool_result placeholders eat tokens for no benefit. Replace
        // with a smaller marker so the next turn's prompt cache stays
        // healthy. Opt-in via
        // `compact.micro.clear_file_unchanged_stubs_enabled` (default off).
        if self.config.compact.micro.enabled
            && self.config.compact.micro.clear_file_unchanged_stubs_enabled
        {
            let _ =
                history.with_owned_messages(|msgs| coco_compact::clear_file_unchanged_stubs(msgs));
        }

        // Compute message-level stats once and share across the
        // auto-memory fan-out and the auto-compact threshold check
        // below — both read the same post-Step-0.5 history.
        //
        // Precision: `MessageHistory::tokens_with_last_usage` is the
        // cohesive method on the history itself — uses the
        // `LastUsageMarker` set in the previous successful turn as
        // baseline + chars/4 estimate of the tail. When the marker is
        // unset (resume / post-compact / first turn) it falls back to
        // a full walk — same accuracy as the previous chars/4-only path.
        let estimated_tokens = history.tokens_with_last_usage();
        let tool_calls_last_turn =
            coco_messages::count_tool_calls_in_last_assistant_turn(history.as_slice());

        // Bare mode skips the entire post-turn fan-out (promptSuggestion +
        // extractMemories + sessionMemory + autoDream). Used by `--bare`
        // SDK / scripted `-p` invocations that don't want background work
        // after each turn.
        let bare_mode_active = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode);

        // Auto-memory turn-end fan-out — black-boxed through
        // `MemoryRuntime::finalize_turn`. The engine pre-computes
        // everything that needs `MessageHistory` (cursors, counts,
        // fork closures) and hands them through the context; the
        // runtime does the SM + extract + dream fan-out and returns
        // a typed report. Engine then projects notices into history
        // and acts on the KAIROS rollover signal.
        if let Some(runtime) = self.memory_runtime.clone() {
            let report = self
                .build_memory_finalize_ctx_and_run(
                    history,
                    estimated_tokens,
                    tool_calls_last_turn,
                    bare_mode_active,
                    &runtime,
                )
                .await;
            for warning in report.index_warnings {
                let bounded = truncate_memory_reminder(&warning);
                let msg = coco_messages::wrapping::create_system_reminder_message_with_kind(
                    coco_types::AttachmentKind::MemoryIndexWarning,
                    &bounded,
                );
                crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
            }
            for update in report.memory_updates {
                let in_context_paths = self.memory_update_in_context_paths(&update.paths).await;
                let reminder = format_memory_update_reminder(&update, &in_context_paths);
                let msg = coco_messages::wrapping::create_system_reminder_message_with_kind(
                    coco_types::AttachmentKind::MemoryUpdateReminder,
                    &reminder,
                );
                crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
            }
            for notice in report.notices {
                let msg =
                    coco_messages::Message::System(coco_messages::SystemMessage::MemorySaved(
                        coco_messages::SystemMemorySavedMessage {
                            uuid: uuid::Uuid::new_v4(),
                            written_paths: notice.written_paths,
                            verb: notice.verb.as_str().to_string(),
                        },
                    ));
                crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
            }
            // KAIROS midnight-rollover signal. The memory crate has
            // already advanced its latch and emitted
            // `MemoryEvent::KairosRollover` telemetry; the engine logs
            // the event under a dedicated target so resume / replay
            // can correlate the day flip with downstream actions.
            // The generic `date_change` system-reminder is independent
            // (it fires for every session via `DateChangeGenerator`),
            // so we don't need to inject a reminder here.
            if let Some(yesterday) = report.kairos_rollover {
                tracing::info!(
                    target: "coco_query::kairos_rollover",
                    yesterday = %yesterday.format("%Y-%m-%d"),
                    session_id = %self.config.session_id,
                    "KAIROS daily-log rollover detected",
                );
            }
        }

        // Skill-learning review — the capability-layer analogue of the memory
        // fan-out above. Gated on `is_terminal()` so the throttle counts
        // user-prompt cycles, not LLM tool-rounds: this tail runs after every
        // tool batch, and an ungated call here would advance the counter each
        // round (firing ~throttle/rounds-per-prompt times too often) and could
        // snapshot a mid-turn history. The no-tool-calls terminal tail covers
        // text-only endings; the two tails are mutually exclusive per round,
        // so each delivered cycle ticks the throttle exactly once.
        if continuation.is_terminal() {
            self.run_skill_review_finalize(history);
        }

        // Collapse-aware guard: when staged_compact is active it owns
        // the threshold ladder, so proactive autocompact suppresses.
        let collapse_active = self.is_collapse_active();
        let context_window = self.resolved_context_window();
        let max_output_tokens = self.resolved_max_output_tokens();
        let auto_compact_needed = coco_compact::should_auto_compact_guarded_with_collapse(
            estimated_tokens,
            context_window,
            max_output_tokens,
            auto_cfg,
            coco_compact::CompactQuerySource::Other,
            collapse_active,
        );
        if !auto_compact_needed {
            tracing::debug!(
                target: "coco_query::compact_decision",
                estimated_tokens,
                context_window,
                collapse_active,
                "auto-compact check: not needed"
            );
        }
        if auto_compact_needed {
            if let Some(report) = coco_compact::prefix_overflow_check(
                history.as_slice(),
                context_window,
                max_output_tokens,
                auto_cfg,
                /*snip_tokens_freed*/ 0,
            ) {
                warn!(
                    target: "coco_query::compact_decision",
                    prefix_tokens = report.prefix_tokens,
                    threshold_tokens = report.threshold_tokens,
                    total_input_tokens = report.total_input_tokens,
                    messages_estimate = report.messages_estimate,
                    snip_tokens_freed = report.snip_tokens_freed,
                    document_block_count = report.document_block_count,
                    image_block_count = report.image_block_count,
                    would_have_blocked = true,
                    "autocompact: fixed prefix exceeds threshold; compaction may not help"
                );
            }

            // Step 1: threshold micro_compact (count-based). Opt-in via
            // `compact.micro.count_based_enabled` (default off). When off,
            // go straight to SM/LLM compaction below.
            let pre_count = history.len() as i32;
            let pre_micro_tokens = estimated_tokens;
            if self.config.compact.micro.enabled && self.config.compact.micro.count_based_enabled {
                history.with_owned_messages(|msgs| {
                    coco_compact::micro_compact(msgs, micro_keep);
                });
                info!("auto micro-compaction triggered (keep_recent={micro_keep})");
            }
            let removed = (pre_count - history.len() as i32).max(0);
            // After `with_owned_messages` (above), the marker is
            // cleared — `tokens_with_last_usage` falls back to a full
            // walk via the unified content-kind estimator. Same result
            // as the previous `coco_compact::estimate_tokens` call,
            // but keeps the single canonical entry point.
            let post_micro_tokens = history.tokens_with_last_usage();
            let _ = emit_protocol(
                event_tx,
                ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                    removed_messages: removed,
                    summary_tokens: 0,
                    trigger: coco_types::CompactTrigger::Auto,
                    pre_tokens: Some(pre_micro_tokens),
                    post_tokens: Some(post_micro_tokens),
                }),
            )
            .await;

            if removed > 0 {
                // Auto micro_compact mutated tool result content — suppress
                // the false-positive cache-break warning on the next API call.
                let qs = self.query_source_label();
                self.notify_model_cache_deletion(qs).await;
            }

            let still_over_threshold = coco_compact::should_auto_compact_guarded_with_collapse(
                post_micro_tokens,
                context_window,
                max_output_tokens,
                auto_cfg,
                coco_compact::CompactQuerySource::Other,
                collapse_active,
            );
            if still_over_threshold {
                tracing::debug!(
                    target: "coco_query::compact_decision",
                    pre_micro_tokens,
                    post_micro_tokens,
                    removed,
                    "auto-compact: micro insufficient, proceeding to full compact"
                );
            } else {
                tracing::debug!(
                    target: "coco_query::compact_decision",
                    pre_micro_tokens,
                    post_micro_tokens,
                    removed,
                    "auto-compact: micro sufficient, full compact skipped"
                );
            }
            if still_over_threshold {
                let attempt_decision = {
                    let state = self.auto_compact_state.lock().await;
                    state.attempt_decision()
                };

                match attempt_decision {
                    coco_compact::AutoCompactAttemptDecision::FailureBreakerOpen {
                        consecutive_failures,
                    } => {
                        warn!(
                            consecutive_failures,
                            threshold = coco_compact::types::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES,
                            "auto compaction skipped after repeated failures"
                        );
                    }
                    coco_compact::AutoCompactAttemptDecision::RapidRefillBreakerTripped {
                        consecutive_rapid_refills,
                    } => {
                        warn!(
                            consecutive_rapid_refills,
                            turn_window = coco_compact::types::RAPID_REFILL_TURN_WINDOW,
                            "auto compaction skipped after rapid context refill"
                        );
                        let msg = coco_messages::create_assistant_error_message(
                            coco_compact::types::RAPID_REFILL_BREAKER_MESSAGE,
                            None,
                            Some("invalid_request"),
                        );
                        crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
                    }
                    coco_compact::AutoCompactAttemptDecision::Proceed {
                        consecutive_rapid_refills,
                    } => {
                        // Step 2 → 3: SM-first → full LLM. `try_full_compact`
                        // owns the branch internally so manual `/compact`
                        // benefits too.
                        let outcome = self
                            .try_full_compact(
                                history,
                                event_tx,
                                coco_types::CompactTrigger::Auto,
                                /*custom_instructions*/ None,
                            )
                            .await;
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let mut state = self.auto_compact_state.lock().await;
                        match outcome {
                            coco_compact::CompactOutcome::Applied => {
                                state.record_success(now_ms, consecutive_rapid_refills);
                            }
                            coco_compact::CompactOutcome::Failed => state.record_failure(now_ms),
                            coco_compact::CompactOutcome::Skipped => {}
                        }
                    }
                }
            }
        }

        self.finalize_successful_turn_tail(
            history,
            event_tx,
            usage,
            continuation,
            cycle_turn_id,
            stop_reason,
        )
        .await;
    }
}

#[cfg(test)]
#[path = "engine_finalize_turn.test.rs"]
mod tests;
