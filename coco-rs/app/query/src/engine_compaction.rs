//! Full LLM compaction + session-memory short-circuit + manual entry-point.
//!
//! Owns the three "summarize and rewrite history" flows:
//! - [`QueryEngine::run_manual_compact`] — `/compact [instructions]` user entry,
//! - [`QueryEngine::try_full_compact`] — full LLM summarization (+ pre/post-compact hooks
//!   + post-compact attachment re-injection),
//! - [`QueryEngine::try_session_memory_compact`] — pre-extracted memory short-circuit
//!   that rewrites history without an LLM call when it would still fit under the
//!   auto-compact threshold.
//!
//! The reactive (PTL recovery) path and the per-turn auto-compact ladder live in
//! `crate::engine_finalize_turn` because they share the `finalize_turn_post_tools`
//! sequence and emit a different set of `CompactionPhase` events.

#[path = "engine_compaction_full.rs"]
mod full;
use full::*;

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use tracing::info;
use tracing::warn;

use coco_compact::COMPACT_SUMMARY_ABORTED_PREFIX;
use coco_compact::COMPACT_SUMMARY_INVALID_PREFIX;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_messages::AssistantContent;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::MessageOrigin;
use coco_messages::UserMessage;

use crate::CoreEvent;
use crate::ServerNotification;
use crate::emit::emit_protocol;
use crate::engine::QueryEngine;

const COMPACT_QUERY_SOURCE: &str = "compact";

/// Manual `/compact` invocation context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCompactRequest {
    /// Optional user instructions passed to the summarizer.
    pub custom_instructions: Option<String>,
    /// Raw slash-command argument string used for transcript breadcrumbs.
    pub command_args: String,
}

impl ManualCompactRequest {
    pub fn new(custom_instructions: Option<String>) -> Self {
        let command_args = custom_instructions.clone().unwrap_or_default();
        Self {
            custom_instructions,
            command_args,
        }
    }
}

impl QueryEngine {
    /// Replace the authoritative in-memory history after compaction and keep
    /// the append-only transcript resumable.
    ///
    /// Records compact rewrites by appending the new boundary/summary
    /// chain, not by rewriting JSONL. When the boundary carries a preserved
    /// segment, the preserved tail must already exist on disk so resume can
    /// validate and relink it.
    async fn replace_history_after_compact(
        &self,
        history: &mut MessageHistory,
        new_messages: Vec<Arc<Message>>,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        let has_preserved_segment = new_messages.iter().any(|msg| {
            matches!(
                msg.as_ref(),
                Message::System(coco_messages::SystemMessage::CompactBoundary(boundary))
                    if boundary.preserved_segment.is_some()
            )
        });
        if has_preserved_segment {
            self.record_transcript_tail(history).await;
        }

        crate::history_sync::history_replace_and_emit(
            history,
            new_messages,
            event_tx,
            coco_types::HistoryReplaceReason::Compact,
        )
        .await;
        self.record_transcript_tail(history).await;
    }

    /// Public manual entry-point for `/compact [instructions]`.
    ///
    /// Equivalent to the auto path but with `CompactTrigger::Manual` and
    /// the user-supplied instructions threaded into the summary prompt.
    /// Callers (TUI / SDK) can drive compaction directly without going
    /// through the auto-trigger threshold.
    pub async fn run_manual_compact(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        request: ManualCompactRequest,
    ) -> coco_compact::CompactOutcome {
        // `COCO_COMPACT_DISABLE` removes /compact from the registry
        // entirely, disabling BOTH auto and manual compaction. Honor the
        // hard-kill here so the manual path (SDK / scripted / old
        // transcript) can't bypass it.
        if self.config.compact.auto.disabled_by_env {
            append_manual_compact_notice(
                history,
                event_tx,
                &request,
                "Compaction is disabled via the COCO_COMPACT_DISABLE environment variable.",
            )
            .await;
            emit_compaction_done(event_tx).await;
            return coco_compact::CompactOutcome::Skipped;
        }
        if history.is_empty() {
            append_manual_compact_notice(history, event_tx, &request, "No messages to compact.")
                .await;
            emit_compaction_done(event_tx).await;
            return coco_compact::CompactOutcome::Skipped;
        }

        // Non-empty manual `/compact` histories flow through
        // session-memory / LLM compaction; the compact service decides
        // whether there is enough conversation to summarize.

        // Micro-compact runs before `compactConversation` only when
        // count-based mode is enabled (default off) — the time-based
        // path does not fire synchronously here.
        // Opt-in via `compact.micro.count_based_enabled`; when off,
        // go straight to SM/LLM.
        let micro_keep = self.config.compact.micro.keep_recent.max(0) as usize;
        let will_try_sm =
            request.custom_instructions.is_none() && self.config.compact.session_memory.enabled;
        if !will_try_sm
            && self.config.compact.micro.enabled
            && self.config.compact.micro.count_based_enabled
        {
            history.with_owned_messages(|msgs| {
                coco_compact::micro_compact(msgs, micro_keep);
            });
        }

        // SM-first short-circuit + LLM fallback are both centralized in
        // `try_full_compact` — manual path just passes the trigger and
        // any custom instructions through. When custom instructions are
        // present we want the LLM path; `try_full_compact` already
        // skips SM in that case (see its branch).
        self.try_full_compact_impl(
            history,
            event_tx,
            coco_types::CompactTrigger::Manual,
            request.custom_instructions.clone(),
            Some(&request),
        )
        .await
    }

    pub async fn run_partial_compact(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        pivot_index: usize,
        direction: coco_messages::PartialCompactDirection,
        user_feedback: Option<String>,
        custom_instructions: Option<String>,
    ) -> coco_compact::CompactOutcome {
        let snapshot = if let Some(frs) = &self.file_read_state {
            let frs = frs.read().await;
            frs.snapshot_by_recency()
        } else {
            Vec::new()
        };
        let captured_skills = self
            .post_compact_skills
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let captured_plan_mode_snapshot = self.snapshot_plan_mode_attachment().await;
        let prioritized_paths = self.recently_mentioned_paths_snapshot().await;

        let hook_trigger = coco_hooks::orchestration::CompactTrigger::Manual;
        let _ = emit_protocol(
            event_tx,
            ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::HooksStart,
                hook_type: Some(coco_types::CompactionHookType::PreCompact),
            }),
        )
        .await;

        let mut effective_instructions = custom_instructions.clone();
        let mut pre_display: Option<String> = None;
        if let Some(registry) = self.hooks_for(coco_types::HookEventType::PreCompact) {
            let ctx = self.orchestration_ctx();
            match coco_hooks::orchestration::execute_pre_compact(
                registry,
                &ctx,
                hook_trigger,
                custom_instructions.as_deref(),
            )
            .await
            {
                Ok(res) => {
                    effective_instructions = coco_compact::merge_hook_instructions(
                        effective_instructions.as_deref(),
                        res.new_custom_instructions.as_deref(),
                    );
                    pre_display = res.user_display_message;
                }
                Err(e) => warn!("PreCompact hook execution failed (partial compact): {e}"),
            }
        }

        let _ = emit_protocol(
            event_tx,
            ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::Summarizing,
                hook_type: None,
            }),
        )
        .await;

        let summarize_fn = |attempt: coco_compact::CompactSummaryAttempt| async move {
            self.run_compact_summary_attempt(attempt, event_tx).await
        };
        let result = coco_compact::partial_compact_conversation(
            history.as_slice(),
            pivot_index,
            direction,
            user_feedback.as_deref(),
            effective_instructions.as_deref(),
            summarize_fn,
            None,
        )
        .await;

        match result {
            Ok(mut result) => {
                if let Some(msg) = pre_display.as_ref() {
                    result.user_display_message = Some(match result.user_display_message {
                        Some(prev) => format!("{prev}\n{msg}"),
                        None => msg.clone(),
                    });
                }

                let fallback_summary = result
                    .summary_messages
                    .iter()
                    .filter_map(coco_compact::summary_text::extract_message_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                let summary_text = result.raw_summary.as_deref().unwrap_or(&fallback_summary);
                let _ = emit_protocol(
                    event_tx,
                    ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                        phase: coco_types::CompactionPhase::HooksStart,
                        hook_type: Some(coco_types::CompactionHookType::PostCompact),
                    }),
                )
                .await;
                if let Some(registry) = self.hooks_for(coco_types::HookEventType::PostCompact) {
                    let ctx = self.orchestration_ctx();
                    match coco_hooks::orchestration::execute_post_compact(
                        registry,
                        &ctx,
                        hook_trigger,
                        summary_text,
                    )
                    .await
                    {
                        Ok(res) => {
                            if let Some(msg) = res.user_display_message {
                                result.user_display_message =
                                    Some(match result.user_display_message {
                                        Some(prev) => format!("{prev}\n{msg}"),
                                        None => msg,
                                    });
                            }
                        }
                        Err(e) => warn!("PostCompact hook execution failed (partial compact): {e}"),
                    }
                }

                let cwd = self.config.workspace_cwd();
                let plan_file = self.config_home.as_ref().map(|ch| {
                    let plans_dir = coco_context::resolve_plans_directory(
                        ch,
                        self.config.project_dir.as_deref(),
                        self.config.plans_directory.as_deref(),
                    );
                    coco_context::get_plan_file_path(
                        self.session_id.as_str(),
                        &plans_dir,
                        self.config.agent_id_str(),
                    )
                });
                let max_files_to_restore =
                    self.config.compact.post_compact.max_files_to_restore.max(0) as usize;
                result.attachments.extend(
                    coco_compact::create_post_compact_file_attachments_with_priority_and_limit(
                        &snapshot,
                        &result.messages_to_keep,
                        &cwd,
                        plan_file.as_deref(),
                        &prioritized_paths,
                        max_files_to_restore,
                    ),
                );
                if let Some(att) = self.create_current_plan_attachment() {
                    result.attachments.push(att);
                }
                result
                    .attachments
                    .extend(coco_compact::create_post_compact_skill_attachments(
                        &captured_skills,
                    ));
                if let Some(pm) = captured_plan_mode_snapshot
                    && let Some(att) = coco_compact::create_plan_mode_attachment_if_needed(true, pm)
                {
                    result.attachments.push(att);
                }
                if let Some(registry) = self.hooks_for(coco_types::HookEventType::SessionStart) {
                    let _ = emit_protocol(
                        event_tx,
                        ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                            phase: coco_types::CompactionPhase::HooksStart,
                            hook_type: Some(coco_types::CompactionHookType::SessionStart),
                        }),
                    )
                    .await;
                    result.hook_results.extend(
                        self.compact_session_start_hook_messages(registry, "partial compact")
                            .await,
                    );
                }

                let (delta_attachments, delta_state) = self
                    .create_post_compact_delta_attachments(&result.messages_to_keep)
                    .await;
                result.attachments.extend(delta_attachments);
                let new_messages =
                    coco_compact::build_partial_post_compact_messages(&result, direction);
                let pre_len = history.len() as i32;
                let post_len = new_messages.len() as i32;
                let removed_messages = (pre_len - post_len).max(0);
                // I-1 (Authority): partial compaction rewrites the
                // engine-authoritative history. Pair the swap with a
                // `MessageTruncated { 0 }` + per-message
                // `MessageAppended` burst so the TUI's TranscriptView
                // and SDK observers see the new state.
                self.replace_history_after_compact(history, new_messages.clone(), event_tx)
                    .await;
                self.update_post_compact_delta_state(delta_state).await;
                if let Some(frs) = &self.file_read_state {
                    let mut frs = frs.write().await;
                    frs.clear();
                }
                let is_main_agent = self.config.agent_id.is_none();
                self.compaction_observers
                    .notify_all(&result, is_main_agent)
                    .await;
                self.compaction_observers
                    .notify_post_compact(&new_messages)
                    .await;
                let qs = self.query_source_label();
                self.notify_model_compaction(qs).await;
                // See `try_session_memory_compact` for the matching
                // reset on the SM-first path. The partial-compact path
                // rewrites history just like full LLM compact does, so
                // dropped attachment messages must reset the recall
                // state's already-surfaced + 60 KB byte budget AND
                // clear the SM in-memory cache so the next SM-first
                // short-circuit doesn't read pre-compact content.
                if let Some(rt) = &self.memory_runtime {
                    rt.reset_recall_state();
                    rt.session_memory.clear_after_compact().await;
                }
                let _ = emit_protocol(
                    event_tx,
                    ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                        removed_messages,
                        summary_tokens: result.post_compact_tokens as i32,
                        trigger: coco_types::CompactTrigger::Manual,
                        pre_tokens: Some(result.pre_compact_tokens),
                        post_tokens: Some(result.post_compact_tokens),
                    }),
                )
                .await;
                let _ = emit_protocol(
                    event_tx,
                    ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                        phase: coco_types::CompactionPhase::Done,
                        hook_type: None,
                    }),
                )
                .await;
                self.pending_just_compacted
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                coco_compact::CompactOutcome::Applied
            }
            Err(e) => {
                warn!("partial compaction failed: {e}");
                let _ = emit_protocol(
                    event_tx,
                    ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                        phase: coco_types::CompactionPhase::Done,
                        hook_type: None,
                    }),
                )
                .await;
                coco_compact::CompactOutcome::Failed
            }
        }
    }

    /// Try the session-memory-first compact path. Returns `true` when SM
    /// produced a result and history was rewritten; `false` when the
    /// caller should fall through to LLM summarization.
    ///
    /// The SM path bypasses PreCompact / PostCompact hooks (only
    /// sessionStart hooks fire) — context recovery is already in the
    /// memory text itself.
    pub(crate) async fn try_session_memory_compact(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        outer_trigger: coco_types::CompactTrigger,
        manual_request: Option<&ManualCompactRequest>,
    ) -> bool {
        // Wait for any in-flight forked-agent extraction so we don't
        // snapshot an about-to-be-overwritten memory file.
        // Past `STALE_THRESHOLD` (60s) the call returns false and we
        // proceed — extraction is presumed crashed.
        if let Some(svc) = &self.session_memory_service {
            let _ = svc
                .wait_for_extraction(coco_memory::service::session::DEFAULT_WAIT_TIMEOUT)
                .await;
        }

        // Prefer the service's cached body — refreshed inside
        // `run_with_label` after each successful extract. Falls
        // back to the engine-local text (legacy / test path) when
        // the service isn't wired.
        let memory_text = if let Some(svc) = &self.session_memory_service {
            svc.current_text().await
        } else {
            self.session_memory_text.read().await.clone()
        };
        if memory_text.trim().is_empty() {
            return false;
        }

        // Build the SM compact config from the resolved settings, threading
        // the auto-compact threshold so compaction declines when the result
        // wouldn't actually shrink below the line.
        let sm_cfg = &self.config.compact.session_memory;
        let auto_threshold = coco_compact::auto_compact_threshold(
            self.resolved_context_window(),
            self.resolved_max_output_tokens(),
            &self.config.compact.auto,
        );
        let path_str = self
            .config_home
            .as_ref()
            .map(|p| format!("{}/session-memory/summary.md", p.display()));
        let sm_compact_cfg = coco_compact::SessionMemoryCompactConfig {
            min_tokens: sm_cfg.min_tokens,
            min_text_block_messages: sm_cfg.min_text_block_messages,
            max_tokens: sm_cfg.max_tokens,
            auto_compact_threshold: Some(auto_threshold),
            max_summary_chars: Some(sm_cfg.max_summary_chars as usize),
            session_memory_path: path_str,
        };

        // Read the boundary anchor. Prefer the service's value when
        // installed — the extractor writes it there on each successful
        // extract. Fall back to the engine-local Mutex for tests / SDK
        // paths that bypass the service. Sync the local cache so
        // subsequent reads agree.
        let last_summarized = if let Some(svc) = &self.session_memory_service {
            let from_svc = svc.last_summarized_message_uuid().await;
            if let Some(uuid) = from_svc
                && let Ok(mut guard) = self.last_summarized_message_id.lock()
            {
                *guard = Some(uuid);
            }
            from_svc.or_else(|| self.last_summarized_message_id.lock().ok().and_then(|g| *g))
        } else {
            self.last_summarized_message_id.lock().ok().and_then(|g| *g)
        };

        let mut result = match coco_compact::compact_session_memory(
            history.as_slice(),
            &memory_text,
            last_summarized,
            &sm_compact_cfg,
        ) {
            Ok(Some(r)) => r,
            Ok(None) => return false, // SM declined; fall through to LLM.
            Err(e) => {
                warn!("session-memory compaction errored: {e}");
                return false;
            }
        };

        // Run SessionStart hooks and insert their rendered attachment
        // messages into the rewritten history. We collect events directly
        // instead of also pushing them to the next-turn sync buffer, so
        // compact output is not delivered twice.
        if let Some(registry) = self.hooks_for(coco_types::HookEventType::SessionStart) {
            let _ = emit_protocol(
                event_tx,
                ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                    phase: coco_types::CompactionPhase::HooksStart,
                    hook_type: Some(coco_types::CompactionHookType::SessionStart),
                }),
            )
            .await;
            result.hook_results.extend(
                self.compact_session_start_hook_messages(registry, "session-memory compact")
                    .await,
            );
        }

        info!(
            pre = result.pre_compact_tokens,
            post = result.post_compact_tokens,
            outer_trigger = ?outer_trigger,
            "session-memory compaction applied",
        );

        // FileReadState clear (same as the LLM path).
        if let Some(frs) = &self.file_read_state {
            let mut frs = frs.write().await;
            frs.clear();
        }

        // Update lastSummarizedMessageId to the *new* boundary anchor —
        // the last kept assistant message's uuid (or None when no
        // assistants survived). Mirror to the service so the next
        // extraction sees the same anchor.
        let new_anchor = result
            .messages_to_keep
            .iter()
            .rev()
            .find(|m| matches!(m.as_ref(), coco_messages::Message::Assistant(_)))
            .and_then(|m| m.uuid())
            .copied();
        if let Ok(mut guard) = self.last_summarized_message_id.lock() {
            *guard = new_anchor;
        }
        if let Some(svc) = &self.session_memory_service {
            svc.set_last_summarized_message_id(new_anchor).await;
        }

        let summary_tokens = result.post_compact_tokens as i32;
        let pre_tokens = result.pre_compact_tokens;
        let post_tokens = result.post_compact_tokens;
        if let Some(att) = self.create_current_plan_attachment() {
            result.attachments.push(att);
        }
        if let Some(pm) = self.snapshot_plan_mode_attachment().await
            && let Some(att) = coco_compact::create_plan_mode_attachment_if_needed(true, pm)
        {
            result.attachments.push(att);
        }
        if let Some(request) = manual_request {
            append_manual_compact_breadcrumbs(&mut result, request);
        }
        let (delta_attachments, delta_state) = self
            .create_post_compact_delta_attachments(&result.messages_to_keep)
            .await;
        result.attachments.extend(delta_attachments);
        let new_messages = coco_compact::build_post_compact_messages(&result);
        let pre_len = history.len() as i32;
        let post_len = new_messages.len() as i32;
        let removed_messages = (pre_len - post_len).max(0);
        // I-1 (Authority): session-memory compaction rewrites history.
        // Emit truncate + appended-burst so the TUI/SDK derived views
        // converge on the new state.
        self.replace_history_after_compact(history, new_messages.clone(), event_tx)
            .await;
        self.update_post_compact_delta_state(delta_state).await;

        let _ = emit_protocol(
            event_tx,
            ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                removed_messages,
                summary_tokens,
                trigger: coco_types::CompactTrigger::SessionMemory,
                pre_tokens: Some(pre_tokens),
                post_tokens: Some(post_tokens),
            }),
        )
        .await;

        // Notify post-compact observers (file caches, permissions, …).
        // `is_main_agent = config.agent_id.is_none()`: subagents must not
        // wipe main-thread DenialTracker / ToolAppState — those are
        // owned by the parent.
        let is_main_agent = self.config.agent_id.is_none();
        self.compaction_observers
            .notify_all(&result, is_main_agent)
            .await;
        self.compaction_observers
            .notify_post_compact(&new_messages)
            .await;
        // Reset the cache-break baseline so the post-compact drop in
        // cache_read tokens doesn't false-positive as a break.
        let qs = self.query_source_label();
        self.notify_model_compaction(qs).await;
        // Reset the memory recall state. The dedup set and 60 KB
        // budget are persisted on the runtime; without this explicit
        // reset the post-compact session would inherit a saturated byte
        // budget and never re-surface memory.
        //
        // Also clear SM's in-memory text cache so the next extract
        // re-reads the file fresh. Without this the SM-first compact
        // short-circuit could serve a stale cached body to the next
        // compaction.
        if let Some(rt) = &self.memory_runtime {
            rt.reset_recall_state();
            rt.session_memory.clear_after_compact().await;
        }
        let _ = emit_protocol(
            event_tx,
            ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::Done,
                hook_type: None,
            }),
        )
        .await;
        // Surface task_status reminders on the next turn.
        self.pending_just_compacted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        true
    }

    async fn run_compact_summary_attempt(
        &self,
        attempt: coco_compact::CompactSummaryAttempt,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) -> Result<coco_compact::CompactSummaryResponse, String> {
        if self.cancel.is_cancelled() {
            return Err(format!("{COMPACT_SUMMARY_ABORTED_PREFIX} cancelled"));
        }

        if let Some(dispatcher) = self.fork_dispatcher.clone() {
            let mut cache = self.last_cache_safe_params().await.unwrap_or_else(|| {
                let snapshot = self.runtime_snapshot();
                let provider = snapshot
                    .as_ref()
                    .map(|s| s.provider.clone())
                    .unwrap_or_default();
                let slot_effort = snapshot.and_then(|s| s.role_effort);
                Self::cache_safe_params_from_parts(
                    &self.config,
                    provider,
                    slot_effort,
                    &coco_messages::MessageHistory::new(),
                )
            });
            // `CompactSummaryAttempt.context_messages` is already
            // `Vec<Arc<Message>>` — Arc-share into the fork context.
            cache.fork_context_messages = attempt.context_messages.clone();

            let mut options =
                crate::forked_agent::ForkedAgentOptions::for_label(coco_types::ForkLabel::Compact);
            options.transcript_mode = crate::forked_agent::ForkTranscriptMode::Sidechain;
            options.can_use_tool = Some(crate::forked_agent::deny_all_handle(
                "compact summary: tools disabled",
            ));
            options.require_can_use_tool = true;
            options.fallback_min_context_window = self.compact_fallback_min_context_window();
            options.overrides.abort = Some(self.cancel.clone());

            let dispatch_started = std::time::Instant::now();
            match dispatcher
                .dispatch(&cache, &options, &attempt.summary_request, None)
                .await
            {
                Ok(result) => {
                    // The summarizer call is real session spend even when
                    // the summary turns out unusable — record it into the
                    // session usage tracker (no-op on engines without one:
                    // forks / subagents account through their own trackers).
                    if result.total_usage.total() > 0 {
                        self.record_session_usage(
                            event_tx,
                            &cache.provider,
                            &cache.model_id,
                            result.total_usage,
                            dispatch_started.elapsed().as_millis() as i64,
                            coco_types::UsageSource::Compact,
                        )
                        .await;
                    }
                    match extract_compact_summary_from_messages(&result.messages, &self.cancel) {
                        Ok(summary) => {
                            return Ok(coco_compact::CompactSummaryResponse { summary });
                        }
                        Err(e) => {
                            warn!("compact fork returned unusable summary: {e}");
                        }
                    }
                }
                Err(e) => {
                    warn!("compact fork failed, falling back to direct no-tools call: {e}");
                }
            }
        }

        self.run_direct_compact_summary_attempt(attempt, event_tx)
            .await
    }

    async fn compact_session_start_hook_messages(
        &self,
        registry: &coco_hooks::HookRegistry,
        context_label: &str,
    ) -> Vec<Message> {
        let ctx = self.orchestration_ctx();
        let model_id = self.config.model_id.as_str();
        let model_arg = if model_id.is_empty() {
            None
        } else {
            Some(model_id)
        };
        match coco_hooks::orchestration::execute_session_start_collect_events(
            registry,
            &ctx,
            coco_hooks::orchestration::SessionStartSource::Compact,
            /*agent_type*/ None,
            model_arg,
        )
        .await
        {
            Ok(result) => {
                let effects = crate::session_start_hooks::SessionStartHookSideEffects::from(
                    &result.aggregate,
                );
                if let Some(sink) = &self.session_start_hook_side_effect_sink {
                    sink.handle_session_start_hook_side_effects(effects.clone())
                        .await;
                }

                let mut messages = self.render_session_start_hook_events(result.events).await;
                if let Some(initial) = effects.initial_user_message {
                    messages.push(coco_messages::create_user_message(&initial));
                }
                messages
            }
            Err(e) => {
                warn!("SessionStart hook execution failed ({context_label}): {e}");
                Vec::new()
            }
        }
    }

    async fn render_session_start_hook_events(
        &self,
        events: Vec<coco_system_reminder::HookEvent>,
    ) -> Vec<Message> {
        if events.is_empty() {
            return Vec::new();
        }

        let ctx = coco_system_reminder::GeneratorContextBuilder::new(&self.config.system_reminder)
            .hook_events(events)
            .build();
        let mut reminders = Vec::new();
        for generated in [
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::HookSuccessGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::HookBlockingErrorGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::HookAdditionalContextGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::HookStoppedContinuationGenerator,
                &ctx,
            )
            .await,
        ] {
            match generated {
                Ok(Some(reminder)) => reminders.push(reminder),
                Ok(None) => {}
                Err(e) => warn!("compact session-start hook reminder generation failed: {e}"),
            }
        }

        // Compact-side reminders go into a scratch vector (no
        // MessageHistory yet); event emission is not relevant here
        // because the engine hasn't started the next turn. Just
        // collect the materialized model-visible messages.
        coco_system_reminder::inject_reminders(reminders).model_visible
    }

    async fn run_direct_compact_summary_attempt(
        &self,
        attempt: coco_compact::CompactSummaryAttempt,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) -> Result<coco_compact::CompactSummaryResponse, String> {
        if self.cancel.is_cancelled() {
            return Err(format!("{COMPACT_SUMMARY_ABORTED_PREFIX} cancelled"));
        }

        let mut prompt = coco_messages::normalize_messages_for_api(&attempt.context_messages);
        prompt.push(LlmMessage::user_text(&attempt.summary_request));

        let source = ModelRuntimeSource::Role(coco_types::ModelRole::Main);
        let fallback_min_context_window = self.compact_fallback_min_context_window();
        let moa_turn_id = coco_types::TurnId::generate();

        loop {
            let params = compact_summary_query_params(
                prompt.clone(),
                attempt.max_summary_tokens,
                fallback_min_context_window,
                Some(&self.session_id),
            );
            let params = crate::moa::maybe_attach_moa_guidance_for_query_once(
                &self.model_runtimes,
                &source,
                &params,
                event_tx,
                &moa_turn_id,
                crate::moa::MoaReferenceUsageRecorder::Engine(self),
            )
            .await;
            let call_started = std::time::Instant::now();
            match self
                .model_runtimes
                .query_once(source.clone(), &params)
                .await
            {
                ModelRuntimeQueryOutcome::Success {
                    result, snapshot, ..
                } => {
                    // Record spend before the usability checks below — a
                    // truncated / unparsable summary was still billed.
                    if result.usage.total() > 0 {
                        self.record_session_usage(
                            event_tx,
                            &snapshot.provider,
                            &snapshot.model_id,
                            result.usage,
                            call_started.elapsed().as_millis() as i64,
                            coco_types::UsageSource::Compact,
                        )
                        .await;
                    }
                    let stop = result.stop_reason.as_ref();
                    let stop_abnormal = stop.is_some_and(coco_messages::FinishReason::is_abnormal);
                    // A truncated / content-filtered / refused summary is
                    // unusable — it would silently contaminate every
                    // subsequent turn with partial XML. Return an `Err`
                    // carrying `COMPACT_SUMMARY_ABORTED_PREFIX`; the
                    // prefix match in `coco_compact::call_with_ptl_retry`
                    // routes it into `CompactError::LlmCallFailed`, which
                    // the user sees as "Error compacting conversation".
                    // Multi-provider note: some providers convert `max_tokens`
                    // into a synthetic API-error message at the stream layer;
                    // coco-rs does not, so the side-fork caller has to
                    // inspect `stop_reason` directly here.
                    if stop_abnormal {
                        warn!(
                            stop_reason = ?stop,
                            tokens_out = result.usage.output_tokens.total,
                            "compaction aborted: non-normal stop_reason — \
                             dropping truncated summary to avoid contaminating future turns"
                        );
                        return Err(format!(
                            "{COMPACT_SUMMARY_ABORTED_PREFIX} model stopped with stop_reason={} \
                         (truncated or filtered summary discarded)",
                            stop.map(|f| f.unified.as_wire_str()).unwrap_or("unknown")
                        ));
                    }
                    let summary_res = extract_compact_summary_from_content(&result.content);
                    if summary_res.is_err() {
                        warn!(
                            stop_reason = ?stop,
                            tokens_out = result.usage.output_tokens.total,
                            "compaction summary parse failed — XML extractor rejected response"
                        );
                    }
                    let summary = summary_res?;
                    break Ok(coco_compact::CompactSummaryResponse { summary });
                }
                ModelRuntimeQueryOutcome::Retry { events } => {
                    log_compact_fallback_events(COMPACT_QUERY_SOURCE, &events);
                    continue;
                }
                ModelRuntimeQueryOutcome::Failed { error, .. } => break Err(error.to_string()),
            }
        }
    }

    fn compact_fallback_min_context_window(&self) -> Option<i64> {
        self.model_runtimes
            .primary_model_info_for_source(ModelRuntimeSource::Role(coco_types::ModelRole::Main))
            .ok()
            .flatten()
            .map(|info| i64::from(info.context_window))
    }

    async fn create_post_compact_delta_attachments<M: std::borrow::Borrow<Message>>(
        &self,
        preserved_history: &[M],
    ) -> (Vec<coco_messages::AttachmentMessage>, PostCompactDeltaState) {
        let app_state_snapshot = match &self.app_state {
            Some(state) => state.read().await.clone(),
            None => coco_types::ToolAppState::default(),
        };

        let current_tool_materialization =
            self.current_tool_materialization(&app_state_snapshot).await;
        let mut current_loaded_tools: Vec<String> = current_tool_materialization
            .loaded()
            .map(|tool| tool.wire_name.as_str().to_string())
            .collect();
        current_loaded_tools.sort();
        let mut current_deferred_tools: Vec<String> = current_tool_materialization
            .deferred()
            .map(|tool| tool.wire_name.as_str().to_string())
            .collect();
        current_deferred_tools.sort();
        let current_agents = self.current_agent_types();
        let source_timeout =
            std::time::Duration::from_millis(if self.config.system_reminder.timeout_ms > 0 {
                self.config.system_reminder.timeout_ms as u64
            } else {
                coco_system_reminder::DEFAULT_TIMEOUT_MS as u64
            });
        let materialized = self
            .reminder_sources
            .materialize(coco_system_reminder::MaterializeContext {
                config: &self.config.system_reminder,
                agent_id: self.config.agent_id_str(),
                user_input: None,
                mentioned_paths: &[],
                recent_tools: &[],
                just_compacted: true,
                per_source_timeout: source_timeout,
                skill_overrides: &self.config.skill_overrides,
                skill_tool_loaded: false,
            })
            .await;
        let mut visible_mcp_tool_counts = std::collections::BTreeMap::<String, usize>::new();
        for tool in current_tool_materialization.searchable() {
            if let Some(info) = tool.tool.mcp_info() {
                *visible_mcp_tool_counts
                    .entry(info.server_name.clone())
                    .or_default() += 1;
            }
        }
        let current_mcp_servers: Vec<_> = materialized
            .mcp_server_summaries
            .iter()
            .filter_map(|server| {
                visible_mcp_tool_counts
                    .get(&server.name)
                    .copied()
                    .map(|tool_count| coco_system_reminder::McpServerSummary {
                        name: server.name.clone(),
                        tool_count,
                        description: server.description.clone(),
                    })
            })
            .collect();
        let current_mcp_server_state: std::collections::BTreeMap<_, _> = current_mcp_servers
            .iter()
            .map(|server| {
                (
                    server.name.clone(),
                    coco_types::McpServerAnnouncementState {
                        tool_count: server.tool_count,
                        description: server.description.clone(),
                    },
                )
            })
            .collect();
        let current_mcp_instructions = materialized.mcp_instructions_current;

        let baseline_tools = if preserved_contains_attachment_kind(
            preserved_history,
            coco_types::AttachmentKind::DeferredToolsDelta,
        ) {
            app_state_snapshot.last_announced_tools_for_scope(self.config.agent_id_str())
        } else {
            HashSet::new()
        };
        let baseline_agents = if preserved_contains_attachment_kind(
            preserved_history,
            coco_types::AttachmentKind::AgentListingDelta,
        ) {
            app_state_snapshot.last_announced_agents.clone()
        } else {
            HashSet::new()
        };
        let baseline_mcp = if preserved_contains_attachment_kind(
            preserved_history,
            coco_types::AttachmentKind::McpInstructionsDelta,
        ) {
            app_state_snapshot.last_announced_mcp_instructions.clone()
        } else {
            HashMap::new()
        };
        let baseline_mcp_servers = if preserved_contains_attachment_kind(
            preserved_history,
            coco_types::AttachmentKind::McpServersDelta,
        ) {
            app_state_snapshot.last_announced_mcp_servers_for_scope(self.config.agent_id_str())
        } else {
            std::collections::BTreeMap::new()
        };

        let deferred_delta = crate::engine_helpers::compute_tools_delta(
            &current_deferred_tools,
            &current_loaded_tools,
            &baseline_tools,
        );
        let agent_delta =
            crate::engine_helpers::compute_agents_delta(&current_agents, &baseline_agents);
        let mcp_delta = crate::engine_helpers::compute_mcp_instructions_delta(
            &current_mcp_instructions,
            &baseline_mcp,
        );
        let mcp_servers_delta = crate::engine_helpers::compute_mcp_servers_delta(
            &current_mcp_servers,
            &baseline_mcp_servers,
        );

        let ctx = coco_system_reminder::GeneratorContextBuilder::new(&self.config.system_reminder)
            .deferred_tools_delta(deferred_delta)
            .agent_listing_delta(agent_delta)
            .mcp_instructions_delta(mcp_delta)
            .mcp_servers_delta(mcp_servers_delta)
            .build();
        let mut reminders = Vec::new();
        for generated in [
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::DeferredToolsDeltaGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::AgentListingDeltaGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::McpInstructionsDeltaGenerator,
                &ctx,
            )
            .await,
            coco_system_reminder::AttachmentGenerator::generate(
                &coco_system_reminder::McpServersDeltaGenerator,
                &ctx,
            )
            .await,
        ] {
            match generated {
                Ok(Some(reminder)) => reminders.push(reminder),
                Ok(None) => {}
                Err(e) => warn!("post-compact delta reminder generation failed: {e}"),
            }
        }

        let batch = coco_system_reminder::inject_reminders(reminders);
        let mut attachments = Vec::new();
        for message in batch.model_visible {
            if let Message::Attachment(att) = message {
                attachments.push(att);
            }
        }

        let state = PostCompactDeltaState {
            current_deferred_tools,
            agent_id: self.config.agent_id_string(),
            current_agents,
            current_mcp_instructions,
            current_mcp_servers: current_mcp_server_state,
        };
        (attachments, state)
    }

    async fn current_tool_search_partitions(
        &self,
        app_state: &coco_types::ToolAppState,
    ) -> (Vec<String>, Vec<String>) {
        let materialization = self.current_tool_materialization(app_state).await;
        let mut loaded: Vec<String> = materialization
            .loaded()
            .map(|tool| tool.wire_name.as_str().to_string())
            .collect();
        loaded.sort();
        let mut deferred: Vec<String> = materialization
            .deferred()
            .map(|tool| tool.wire_name.as_str().to_string())
            .collect();
        deferred.sort();
        (loaded, deferred)
    }

    async fn current_tool_materialization(
        &self,
        app_state: &coco_types::ToolAppState,
    ) -> coco_tool_runtime::ToolMaterialization {
        let discovered = std::sync::Arc::new(app_state.discovered_tool_names.clone());
        let snapshot = self.runtime_snapshot();
        let tool_search_strategy =
            crate::tool_context::resolve_tool_search_strategy(snapshot.as_ref());
        let stub_ctx = coco_tool_runtime::ToolUseContext::stub_for_filtering(
            self.config.features.clone(),
            self.config.tool_overrides.clone(),
            self.config.tool_filter.clone(),
            self.config.permission_mode,
        )
        .with_discovered_tool_names(discovered)
        .with_tool_search_strategy(tool_search_strategy)
        .with_mcp_tool_exposure(
            self.config.mcp_tool_exposure,
            self.config.mcp_server_tool_exposure.clone(),
        )
        .with_active_shell_tool(self.config.active_shell_tool);
        self.tools.materialize(&stub_ctx)
    }

    async fn update_post_compact_delta_state(&self, delta_state: PostCompactDeltaState) {
        let Some(app_state) = &self.app_state else {
            return;
        };
        let mut guard = app_state.write().await;
        guard.set_last_announced_tools_for_scope(
            delta_state.agent_id.as_deref(),
            delta_state.current_deferred_tools.into_iter().collect(),
        );
        guard.last_announced_agents = delta_state.current_agents.into_iter().collect();
        guard.last_announced_mcp_instructions = delta_state.current_mcp_instructions;
        guard.set_last_announced_mcp_servers_for_scope(
            delta_state.agent_id.as_deref(),
            delta_state.current_mcp_servers,
        );
    }

    fn create_current_plan_attachment(&self) -> Option<coco_messages::AttachmentMessage> {
        let ch = self.config_home.as_ref()?;
        let plans_dir = coco_context::resolve_plans_directory(
            ch,
            self.config.project_dir.as_deref(),
            self.config.plans_directory.as_deref(),
        );
        let plan_path = coco_context::get_plan_file_path(
            self.session_id.as_str(),
            &plans_dir,
            self.config.agent_id_str(),
        );
        let plan_content = coco_context::get_plan(
            self.session_id.as_str(),
            &plans_dir,
            self.config.agent_id_str(),
        );
        coco_compact::create_plan_attachment_if_needed(&plan_path, plan_content.as_deref())
    }

    fn explore_plan_agents_available(&self, loaded_tools: &[String]) -> bool {
        if !loaded_tools
            .iter()
            .any(|name| name == coco_types::ToolName::Agent.as_str())
        {
            return false;
        }
        let agents = self.current_agent_types();
        agents
            .iter()
            .any(|name| name == coco_types::SubagentType::Explore.as_str())
            && agents
                .iter()
                .any(|name| name == coco_types::SubagentType::Plan.as_str())
    }

    async fn snapshot_plan_mode_attachment(&self) -> Option<coco_compact::PlanModeAttachment> {
        let app_state_snapshot = if let Some(state) = &self.app_state {
            let g = state.read().await;
            g.clone()
        } else {
            coco_types::ToolAppState::default()
        };
        let in_plan_mode = app_state_snapshot.permissions.mode
            == Some(coco_types::PermissionMode::Plan)
            || (self.app_state.is_none()
                && self.config.permission_mode == coco_types::PermissionMode::Plan);
        if !in_plan_mode {
            return None;
        }
        let (loaded_tools, deferred_tools) = self
            .current_tool_search_partitions(&app_state_snapshot)
            .await;

        let workflow = match self.config.plan_mode_settings.workflow {
            coco_config::PlanModeWorkflow::FivePhase => coco_context::PlanWorkflow::FivePhase,
            coco_config::PlanModeWorkflow::Interview => coco_context::PlanWorkflow::Interview,
        };
        let phase4 = match self.config.plan_mode_settings.phase4_variant {
            coco_config::PlanPhase4Variant::Standard => coco_context::Phase4Variant::Standard,
            coco_config::PlanPhase4Variant::Trim => coco_context::Phase4Variant::Trim,
            coco_config::PlanPhase4Variant::Cut => coco_context::Phase4Variant::Cut,
            coco_config::PlanPhase4Variant::Cap => coco_context::Phase4Variant::Cap,
        };
        let (plan_file_path, plan_exists) =
            match (self.config_home.as_deref(), self.session_id.as_str()) {
                (Some(ch), sid) if !sid.is_empty() => {
                    let plans_dir = coco_context::resolve_plans_directory(
                        ch,
                        self.config.project_dir.as_deref(),
                        self.config.plans_directory.as_deref(),
                    );
                    let path = coco_context::get_plan_file_path(
                        sid,
                        &plans_dir,
                        self.config.agent_id_str(),
                    );
                    let exists =
                        coco_context::plan_exists(sid, &plans_dir, self.config.agent_id_str());
                    (path.display().to_string(), exists)
                }
                _ => (String::new(), false),
            };

        Some(coco_compact::PlanModeAttachment {
            reminder_type: coco_context::ReminderType::Full,
            workflow,
            custom_instructions: self.config.plan_mode_settings.custom_instructions.clone(),
            phase4_variant: phase4,
            explore_agent_count: self.config.plan_mode_settings.explore_agent_count,
            plan_agent_count: self.config.plan_mode_settings.plan_agent_count,
            explore_plan_agents_available: self.explore_plan_agents_available(&loaded_tools),
            is_sub_agent: self.config.agent_id.is_some(),
            plan_file_path,
            plan_exists,
            // Model-aware plan-file tool (gpt-5 → apply_patch, Claude → Write/Edit).
            write_tool: self.config.tool_overrides.write_tool(),
            edit_tool: self.config.tool_overrides.edit_tool(),
            deferred_tools,
        })
    }
}

#[cfg(test)]
#[path = "engine_compaction.test.rs"]
mod tests;
