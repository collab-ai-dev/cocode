use super::*;

impl QueryEngine {
    /// Shared successful-turn tail. Persistence (cache snapshot, transcript
    /// flush) and reasoning-metadata side-cache update always run; the
    /// terminal `TurnEnded(Completed)` params are returned only on
    /// [`TurnContinuation::Terminal`]. The session lifecycle wrapper attaches
    /// the final `SessionResultParams` before sending the terminal event.
    /// See [`Self::finalize_turn_post_tools`] for the wire-protocol rationale.
    ///
    /// `cycle_turn_id` is the wire id supplied by the runner; `None`
    /// suppresses the wire emit when the caller had no event channel.
    /// `stop_reason` is `Option<StopReason>` — `None` is preserved
    /// rather than fabricated, so consumers can distinguish "model
    /// returned EndTurn" from "we didn't observe one".
    pub(crate) async fn finalize_successful_turn_tail(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        usage: TokenUsage,
        continuation: TurnContinuation,
        cycle_turn_id: Option<coco_types::TurnId>,
        stop_reason: Option<coco_messages::StopReason>,
    ) -> Option<coco_types::TurnEndedParams> {
        self.flush_successful_turn_state(history).await;
        self.emit_reasoning_metadata_for_last_assistant(event_tx, history, &usage, None)
            .await;
        if continuation.is_terminal()
            && let Some(id) = cycle_turn_id
        {
            return Some(Self::build_turn_ended_completed(
                id,
                usage,
                history.len(),
                stop_reason,
            ));
        }
        None
    }

    /// Build the protocol completion event for a successful model turn.
    ///
    /// Kept distinct from [`Self::finalize_successful_turn_tail`] because
    /// no-tool terminal paths in `run_session_loop` flush + invoke
    /// promptSuggestion + run Stop hooks BEFORE deciding whether to
    /// emit completion (Stop hook may block and re-enter the loop). The
    /// completion invariant still belongs in one place: reasoning
    /// metadata, when reported by the provider, must be anchored by
    /// message UUID before `TurnEnded(Completed)` lets the TUI render
    /// the completed turn.
    pub(crate) async fn finish_successful_turn_completed(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        history: &MessageHistory,
        usage: TokenUsage,
        cycle_turn_id: Option<coco_types::TurnId>,
        stop_reason: Option<coco_messages::StopReason>,
    ) -> Option<coco_types::TurnEndedParams> {
        self.emit_reasoning_metadata_for_last_assistant(event_tx, history, &usage, None)
            .await;
        if let Some(id) = cycle_turn_id {
            return Some(Self::build_turn_ended_completed(
                id,
                usage,
                history.len(),
                stop_reason,
            ));
        }
        None
    }

    /// Persist successful-turn state that must be current before any
    /// post-turn forks read the parent cache slot. Kept separate from
    /// `TurnCompleted` emission so text-only exits can run promptSuggestion
    /// after cache save but before closing the protocol turn.
    pub(crate) async fn flush_successful_turn_state(&self, history: &mut MessageHistory) {
        // D8: snapshot post-turn cache-safe params for engine-owned fork
        // features such as prompt suggestion and compaction.
        // Helper handles the empty-history skip + serialisation.
        self.save_post_turn_cache_params(history).await;

        // Per-turn JSONL transcript append. Walks `history` and writes
        // any user/assistant/system/attachment message whose uuid isn't
        // already in the cross-engine dedup set. Skips silently when
        // the store / session id / dedup set aren't all wired (e.g.
        // tests, headless runs without persistence).
        self.record_transcript_tail(history).await;
    }

    /// Build `TurnEnded(Completed)`.
    ///
    /// - `cycle_turn_id` is the wire-level cycle id (shared with the
    ///   runner's `TurnStarted`).
    /// - `stop_reason` is `Option<StopReason>` because not every
    ///   terminal path has a parsed model finish reason (structured-output
    ///   retry cap, Stop-hook prevent before any round resolved one).
    ///   Emit `None` rather than fabricating `EndTurn`.
    ///
    /// The per-round `turn_id` string is intentionally NOT a parameter —
    /// it's purely log correlation, and `run_session_loop` already stamps
    /// `turn_id` on its per-round info log line. Threading it through
    /// the emit fn just to drop it into one more log line obscured the
    /// wire-vs-log id distinction.
    pub(crate) fn build_turn_ended_completed(
        cycle_turn_id: coco_types::TurnId,
        usage: TokenUsage,
        history_len: usize,
        stop_reason: Option<coco_messages::StopReason>,
    ) -> coco_types::TurnEndedParams {
        info!(
            cycle_turn_id = %cycle_turn_id,
            tokens_in = usage.input_tokens.total,
            tokens_out = usage.output_tokens.total,
            history_len,
            ?stop_reason,
            "turn ended (completed)"
        );
        coco_types::TurnEndedParams::completed(cycle_turn_id, Some(usage), stop_reason)
    }

    /// Emit `ReasoningMetadataAttached` so the TUI side-cache can anchor
    /// reasoning aggregates by the assistant message UUID rather than
    /// re-walking transcript cells. F3 of the unified-transcript plan
    /// — eliminates the prior "find latest AssistantThinking cell"
    /// scan in the TUI handler.
    pub(crate) async fn emit_reasoning_metadata_for_last_assistant(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        history: &MessageHistory,
        usage: &TokenUsage,
        duration_ms: Option<i64>,
    ) {
        if usage.output_tokens.reasoning <= 0 {
            if let Some(assistant) = history.iter().rev().find_map(|m| match m.as_ref() {
                coco_messages::Message::Assistant(a) => Some(a),
                coco_messages::Message::User(_)
                | coco_messages::Message::System(_)
                | coco_messages::Message::Attachment(_)
                | coco_messages::Message::ToolResult(_)
                | coco_messages::Message::Progress(_)
                | coco_messages::Message::Tombstone(_) => None,
            }) && let coco_messages::LlmMessage::Assistant { content, .. } = &assistant.message
            {
                let mut reasoning_chars = 0;
                let mut text_chars = 0;
                let mut tool_call_count = 0;
                for part in content {
                    match part {
                        coco_llm_types::AssistantContentPart::Reasoning(r) => {
                            reasoning_chars += r.text.len();
                        }
                        coco_llm_types::AssistantContentPart::Text(t) => {
                            text_chars += t.text.len();
                        }
                        coco_llm_types::AssistantContentPart::ToolCall(_) => {
                            tool_call_count += 1;
                        }
                        coco_llm_types::AssistantContentPart::File(_)
                        | coco_llm_types::AssistantContentPart::ReasoningFile(_)
                        | coco_llm_types::AssistantContentPart::Custom(_)
                        | coco_llm_types::AssistantContentPart::ToolResult(_)
                        | coco_llm_types::AssistantContentPart::Source(_)
                        | coco_llm_types::AssistantContentPart::ToolApprovalRequest(_) => {}
                    }
                }
                if reasoning_chars > 0 {
                    tracing::debug!(
                        message_uuid = %assistant.uuid,
                        model = %assistant.model,
                        stop_reason = ?assistant.stop_reason,
                        tokens_out = usage.output_tokens.total,
                        text_tokens = usage.output_tokens.text,
                        reasoning_tokens = usage.output_tokens.reasoning,
                        reasoning_chars,
                        text_chars,
                        tool_call_count,
                        "assistant reasoning text present without reasoning token usage"
                    );
                }
            }
            return;
        }
        let Some(last_assistant_uuid) = history.iter().rev().find_map(|m| match m.as_ref() {
            coco_messages::Message::Assistant(a) => Some(a.uuid),
            _ => None,
        }) else {
            return;
        };
        let _ = emit_protocol(
            event_tx,
            ServerNotification::ReasoningMetadataAttached(
                coco_types::ReasoningMetadataAttachedParams {
                    message_uuid: last_assistant_uuid.to_string(),
                    duration_ms,
                    reasoning_tokens: usage.output_tokens.reasoning,
                },
            ),
        )
        .await;
    }

    /// Publish the post-turn message history to two independent sinks:
    ///
    /// 1. The live in-memory snapshot read by the AgentSummary timer —
    ///    refreshed unconditionally whenever a [`LiveTranscript`] sink is
    ///    wired (`with_live_transcript`), regardless of disk persistence.
    /// 2. The durable JSONL transcript — appends every history message whose
    ///    uuid isn't already in the dedup set, with parent_uuid linking to
    ///    the previous message. No-op unless `with_transcript_store` AND
    ///    `with_transcript_dedup` are both wired.
    ///
    /// The two are deliberately decoupled: a sub-agent may have a live reader
    /// without a disk store, while the main loop has the store but no live
    /// reader. Called per turn (via `flush_successful_turn_state`) and on each
    /// compaction boundary, so both sinks track the latest history.
    ///
    /// [`LiveTranscript`]: coco_tool_runtime::LiveTranscript
    pub(crate) async fn record_transcript_tail(&self, history: &MessageHistory) {
        // Publish the post-turn message history to the live sink read by
        // the AgentSummary timer. Independent of the durable transcript
        // store below — a sub-agent may have a live reader without a
        // `transcript_store` wired, and the main loop has the store but
        // no live reader.
        if let Some(live) = self.live_transcript.as_ref() {
            live.set(history.iter().cloned().collect());
        }

        let (Some(store), Some(sid)) = (
            self.transcript_store.as_ref(),
            self.transcript_session_id
                .as_ref()
                .map(coco_types::SessionId::as_str),
        ) else {
            return;
        };

        let cwd_path = self.config.workspace_cwd();
        let cwd = cwd_path.display().to_string();
        // Capture the git branch once per chain and stamp it on every
        // line. Treat a git failure (not in a repo, command missing) as
        // `None` so the field is omitted rather than producing an empty
        // string.
        let git_branch = coco_git::get_current_branch(&cwd_path)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty());
        let now = chrono::Utc::now().to_rfc3339();
        let message_refs: Vec<&coco_messages::Message> =
            history.iter().map(AsRef::as_ref).collect();

        // Mirror the TS upstream: a subagent's turns go to its OWN per-agent
        // transcript (`<sid>/subagents/agent-<id>.jsonl`), never the main
        // session file — so the main transcript never carries sidechain and
        // its parent-uuid chain stays intact. Uses a fresh per-engine dedup so
        // the subagent's full history is persisted even when it shares UUIDs
        // with the main thread (fork-inherited context).
        if let Some(agent_id) = self.config.agent_id_str() {
            let mut seen = self.agent_transcript_dedup.lock().await;
            // Continuity: on the first write for this engine, seed the dedup
            // from the existing per-agent file so a RESUMED run (which reuses
            // the original agent_id and replays its prior history) appends only
            // new turns instead of duplicating what's already on disk. No-op on
            // a fresh spawn (empty/absent file). Mirrors TS seeding
            // `lastRecordedUuid` from the last replayed message.
            if !self
                .agent_transcript_seeded
                .swap(true, std::sync::atomic::Ordering::Relaxed)
                && let Ok(Some(prior)) = store.load_agent_messages(sid, agent_id)
            {
                seen.extend(prior.iter().filter_map(|m| m.uuid().copied()));
            }
            let options = coco_session::storage::ChainWriteOptions {
                cwd,
                timestamp: now,
                is_sidechain: true,
                agent_id: Some(agent_id.to_string()),
                starting_parent_uuid: None,
                git_branch,
            };
            if let Err(e) =
                store.append_agent_message_chain(sid, agent_id, &message_refs, &mut seen, options)
            {
                warn!(error = %e, "failed to append subagent transcript chain");
            }
            return;
        }

        // Main thread: shared per-session dedup, main session transcript file.
        let Some(seen) = self.transcript_dedup.as_ref() else {
            return;
        };
        let mut seen_guard = seen.lock().await;
        let options = coco_session::storage::ChainWriteOptions {
            cwd,
            timestamp: now,
            is_sidechain: false,
            agent_id: None,
            starting_parent_uuid: None,
            git_branch,
        };
        if let Err(e) = store.append_message_chain(sid, &message_refs, &mut seen_guard, options) {
            warn!(error = %e, "failed to append transcript chain");
        }
    }

    /// Seed the tool-result budget / content-replacement state for a RESUMED
    /// subagent from its persisted per-agent records, so the resumed run makes
    /// the SAME budget decisions and replays the exact `<persisted-output>`
    /// strings the model previously saw (byte-identical prompt prefix →
    /// prompt-cache stable). Mirrors TS `reconstructForSubagentResume`:
    /// freeze every replayed tool_use_id into `seen_ids` (the budget never
    /// re-replaces content the model already saw unreplaced) and re-apply the
    /// recorded replacements. MERGE (not overwrite) — the state is shared with
    /// the main thread; `tool_use_id`s are globally unique so entries can't
    /// collide. No-op unless this engine is a subagent with a wired store.
    pub(crate) async fn seed_resumed_replacement_state(
        &self,
        resumed_messages: &[std::sync::Arc<coco_messages::Message>],
    ) {
        let (Some(store), Some(sid), Some(agent_id)) = (
            self.transcript_store.as_ref(),
            self.transcript_session_id
                .as_ref()
                .map(coco_types::SessionId::as_str),
            self.config.agent_id_str(),
        ) else {
            return;
        };
        let records = store
            .load_content_replacements_for_chain(sid, Some(agent_id))
            .unwrap_or_default();
        let mut state = self.tool_result_replacement_state.write().await;
        for msg in resumed_messages {
            if let coco_messages::Message::ToolResult(tr) = msg.as_ref() {
                state.seen_ids.insert(tr.tool_use_id.clone());
            }
        }
        for record in records {
            state.seen_ids.insert(record.tool_use_id().to_string());
            state.replacements.insert(
                record.tool_use_id().to_string(),
                record.replacement().to_string(),
            );
        }
    }

    /// Spawn a `ModelRole::Fast` side-fork to summarize the tool batch
    /// that just completed. Stores the [`tokio::task::JoinHandle`] on
    /// [`QueryEngine::pending_tool_use_summary`] so the await site at
    /// the top of the next `run_session_loop` iteration can drain it.
    ///
    /// Silently no-ops when:
    ///   * `Feature::ToolUseSummary` is disabled (default — see
    ///     `coco_types::features` for the rationale)
    ///   * `model_runtimes` is `None` (no registry wired)
    ///   * `agent_id` is `Some` (subagent skip)
    ///   * `history` has no tool calls in the last assistant turn
    ///     (nothing to summarize)
    ///
    /// Replacing any prior pending handle aborts it first — defense
    /// against orphan tasks if `run_session_loop` skipped its await
    /// (e.g. early cancel between turns).
    pub(crate) async fn spawn_tool_use_summary(&self, history: &MessageHistory) {
        if !self
            .config
            .features
            .enabled(coco_types::Feature::ToolUseSummary)
        {
            return;
        }
        if self.config.agent_id.is_some() {
            return;
        }
        let model_runtimes = self.model_runtimes.clone();
        let Some(input) = crate::tool_use_summary::build_input_from_history(history.as_slice())
        else {
            return;
        };
        if !input.has_tools() {
            return;
        }

        let cancel = self.cancel.clone();
        let usage_accounting = self.usage_accounting.clone();
        let handle = tokio::spawn(async move {
            // Tie the fork to the parent's cancellation. When the user
            // hits Esc, the side-fork doesn't keep running after the
            // turn loop exits.
            tokio::select! {
                _ = cancel.cancelled() => None,
                result = crate::tool_use_summary::generate_tool_use_summary(input, model_runtimes, usage_accounting) => result,
            }
        });

        let mut slot = self.pending_tool_use_summary.lock().await;
        if let Some(prev) = slot.replace(handle) {
            prev.abort();
        }
    }

    /// Drain the pending tool-use-summary fork at the top of a new
    /// iteration. On success, emits `ServerNotification::ToolUseSummary`
    /// for SDK consumers; the TUI side-caches the payload without
    /// writing it to `MessageHistory` (per I-3: tool-use summaries are
    /// UI-only polish and must not pollute the authoritative
    /// transcript). On `None` / join-error, silent skip.
    ///
    /// **No drain-side timeout, no drain-side cancel guard**:
    ///
    /// - The inner [`crate::tool_use_summary::generate_tool_use_summary`]
    ///   caps work via `tokio::time::timeout(10s, …)` which DROPS the
    ///   future on expiry, so the JoinHandle always resolves within
    ///   ~10 s + tiny overhead. Adding a separate (shorter) drain
    ///   timeout would discard summaries that completed at 2–10 s,
    ///   wasting the tokens we already spent.
    /// - Parent cancellation is honored by the spawn's own
    ///   `tokio::select!` on `cancel.cancelled()` — on session cancel
    ///   the inner future is dropped and the handle resolves to
    ///   `Ok(None)` near-instantly. The drain just awaits.
    ///
    /// The expected case is that the Fast-role model resolves during
    /// model streaming so the await is a no-op in practice.
    pub(crate) async fn drain_pending_tool_use_summary(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        let handle = {
            let mut slot = self.pending_tool_use_summary.lock().await;
            slot.take()
        };
        let Some(handle) = handle else {
            return;
        };
        let params = match handle.await {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(join_err) => {
                tracing::debug!(error = %join_err, "tool_use_summary task join error");
                return;
            }
        };

        // Wire-level SDK emission: `tool/useSummary` notification. No
        // transcript entry — UI consumers (TUI) cache the summary by
        // `preceding_tool_use_ids` and render it as overlay polish.
        let _ = emit_protocol(event_tx, ServerNotification::ToolUseSummary(params)).await;
    }

    /// Spawn the post-turn promptSuggestion fork in a detached task
    /// (D2 — production wiring).
    ///
    /// Drives a one-shot fork via [`crate::forked_agent::ForkDispatcher`]
    /// (installed by the CLI bootstrap) using the parent's cached
    /// system prompt + history. The dispatcher builds a *fresh*
    /// engine, so the parent loop is never mutated.
    ///
    /// The suggestion is best-effort: any of these silently skip
    /// recording (the TUI then falls back to the default placeholder):
    /// - no cache slot (first turn hasn't completed)
    /// - no fork dispatcher installed (test / minimal embedding)
    /// - dispatch error (transport crash etc.)
    /// - empty / placeholder-only response from the model
    ///
    /// Only spawns when the assistant did not request follow-up tool
    /// execution (equivalent to the non-bare gate).
    pub(crate) async fn maybe_spawn_prompt_suggestion_after_stop(
        &self,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode) {
            return;
        }
        if self.config.agent_id.is_none()
            && self
                .command_queue
                .has_matching(|cmd| cmd.agent_id.is_none())
                .await
        {
            tracing::debug!("promptSuggestion suppressed because command queue has pending input");
            return;
        }
        let Some(app_state) = self.app_state.as_ref() else {
            return;
        };
        prune_stale_rate_limits(app_state).await;
        self.spawn_prompt_suggestion_task(app_state.clone(), event_tx.clone())
            .await;
    }

    /// Runs a one-shot forked agent with the bespoke suggestion prompt
    /// as a user message; `effort: undefined` preserves cache parity.
    pub(super) async fn spawn_prompt_suggestion_task(
        &self,
        app_state: std::sync::Arc<tokio::sync::RwLock<coco_types::ToolAppState>>,
        event_tx: Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    ) {
        let cache = match self.last_cache_safe_params().await {
            Some(c) => c,
            None => return,
        };
        let dispatcher = match self.fork_dispatcher.clone() {
            Some(d) => d,
            None => return,
        };

        // Build the 9-step `SuggestionContext` from the parent's
        // cache + app_state snapshot BEFORE spawning. The pre-fork
        // guards (TooFewTurns / ApiError / CacheCold / suppress
        // reasons) save the API round-trip when they fire.
        let ctx = build_suggestion_context(
            &cache,
            &app_state,
            self.config.is_non_interactive,
            self.config.is_teammate,
        )
        .await;
        if let Some(outcome) = crate::prompt_suggestion::pre_fork_guards(&ctx, false) {
            tracing::debug!(
                outcome = ?outcome,
                "promptSuggestion suppressed by pre-fork guard"
            );
            return;
        }

        // Cancel any prior in-flight suggestion fork before starting a
        // new one — rapid `/clear` cycles don't accumulate fork tasks
        // burning tokens. Allocate a fresh token, store it under the
        // session-scoped slot, hand a clone to the spawn so the next
        // spawn can cancel cleanly.
        let abort_token = tokio_util::sync::CancellationToken::new();
        if let Some(slot) = self.current_suggestion_abort.as_ref() {
            let mut guard = slot.lock().await;
            if let Some(prev) = guard.replace(abort_token.clone()) {
                prev.cancel();
            }
        }

        // Detach: the suggestion is fire-and-forget. The parent turn is
        // finalizing; we don't want a slow suggestion fork blocking the
        // next user prompt.
        let abort_for_task = abort_token.clone();
        let log_assistant_responses = self.config.log_assistant_responses;
        tokio::spawn(async move {
            // Bail if a newer spawn already cancelled this fork before
            // we got scheduled.
            if abort_for_task.is_cancelled() {
                return;
            }
            // Install deny-all canUseTool so the fork can't actually
            // invoke tools.
            let mut options = crate::forked_agent::ForkedAgentOptions::for_label(
                coco_types::ForkLabel::PromptSuggestion,
            );
            options.can_use_tool = Some(crate::forked_agent::deny_all_handle(
                "prompt suggestion: tools disabled",
            ));
            options.overrides.abort = Some(abort_for_task.clone());
            let prompt = crate::prompt_suggestion::build_suggestion_system_prompt().to_string();
            // The fork sees the parent's system prompt/cache-key
            // params unchanged; the suggestion instruction is appended
            // as the fork's user message.
            let result = dispatcher.dispatch(&cache, &options, &prompt, None).await;
            match result {
                Ok(r) => {
                    // Multi-message text walk — model may loop (try
                    // tool → denied → text in next message). Walks
                    // every assistant message and finds the first
                    // non-empty text block.
                    let generation =
                        crate::prompt_suggestion::extract_suggestion_generation(&r.messages);
                    // Post-fork validation (steps 7-9): aborted /
                    // empty / NONE / 12-rule filter.
                    let aborted_after = abort_for_task.is_cancelled();
                    if let Some(outcome) = crate::prompt_suggestion::post_fork_validation(
                        &generation.text,
                        aborted_after,
                    ) {
                        if let crate::prompt_suggestion::SuggestionOutcome::Filtered { rule } =
                            &outcome
                        {
                            let trimmed = generation.text.trim();
                            let stats = crate::prompt_suggestion::suggestion_text_stats(trimmed);
                            coco_otel::events::emit_prompt_suggestion_filtered(
                                coco_otel::events::PromptSuggestionFilteredPayload {
                                    rule: rule.as_str(),
                                    suggestion_text: trimmed,
                                    text_len_bytes: stats.text_len_bytes,
                                    char_count: stats.char_count,
                                    utf16_len: stats.utf16_len,
                                    word_count: stats.word_count,
                                    cjk_char_count: stats.cjk_char_count,
                                    contains_cjk: stats.contains_cjk,
                                    request_id: generation.request_id.as_deref(),
                                    log_assistant_responses,
                                },
                            );
                        }
                        tracing::debug!(
                            outcome = ?outcome,
                            text_len = generation.text.len(),
                            "promptSuggestion dropped by post-fork validation"
                        );
                        return;
                    }
                    // The live suggestion surfaces via this notification
                    // (TUI folds it into `session.prompt_suggestions`); coco
                    // does not keep a parallel `ToolAppState.prompt_suggestion`
                    // store — see the dropped write-only field.
                    let suggestion = generation.text.trim().to_string();
                    let _delivered = emit_protocol(
                        &event_tx,
                        ServerNotification::PromptSuggestion {
                            suggestions: vec![suggestion],
                        },
                    )
                    .await;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "promptSuggestion fork dispatch failed");
                }
            }
        });
    }

    pub(super) async fn memory_update_in_context_paths(&self, paths: &[String]) -> Vec<String> {
        let read_paths: std::collections::HashSet<std::path::PathBuf> =
            match self.file_read_state.as_ref() {
                Some(frs) => frs
                    .read()
                    .await
                    .iter_entries()
                    .map(|(path, _)| path.to_path_buf())
                    .collect(),
                None => std::collections::HashSet::new(),
            };
        let loaded_nested = self.loaded_nested_memory_paths.lock().await;
        paths
            .iter()
            .filter(|path| {
                let path_buf = std::path::PathBuf::from(path.as_str());
                read_paths.contains(&path_buf) || loaded_nested.contains(&path_buf)
            })
            .cloned()
            .collect()
    }

    /// Fire the turn-end skill-learning review trigger. Inert unless the
    /// runtime was bootstrapped (`Feature::SkillLearning`) and this is a
    /// non-bare, non-subagent turn. The throttle + single-flight + background
    /// spawn all live in `SkillReviewRuntime`; the engine only supplies the
    /// gating context and a lazy history snapshot (built only when firing).
    ///
    /// Called from BOTH turn-end tails — `finalize_turn_post_tools` (gated on
    /// a terminal continuation) and `handle_no_tool_calls_terminal` — which
    /// are mutually exclusive per round, so the throttle ticks once per
    /// delivered user-prompt cycle.
    pub(crate) async fn run_skill_review_finalize(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) {
        let Some(runtime) = self.skill_review_runtime.as_ref() else {
            return;
        };
        if coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode) {
            return;
        }
        // Deliberately re-checked even though the runtime only exists when
        // the feature was on at bootstrap (unlike the memory analogue, which
        // gates on runtime presence alone): engines are rebuilt per turn, so
        // this honors a mid-session settings hot-reload that disables
        // skill-learning.
        if !self
            .config
            .features
            .enabled(coco_types::Feature::SkillLearning)
        {
            return;
        }
        let is_subagent = self.config.agent_id.is_some();
        // A cancelled cycle is an undelivered turn — don't count or review it.
        let turn_delivered = !self.cancel.is_cancelled();
        // L4 signal: fire only when the cycle did material work. Scoped to the
        // whole user cycle, NOT the last assistant round: a substantial cycle
        // ends on a text-only summary round, so a tail-anchored signal reports
        // `tool_calls = 0, skill_invoked = false` for exactly the sessions most
        // worth reviewing — which would leave the throttle permanently unticked.
        let cycle = coco_messages::messages_since_last_user_prompt(history.as_slice());
        let signal = coco_skill_learn::ReviewSignal {
            tool_calls: coco_messages::count_tool_calls_in(cycle),
            skill_invoked: coco_messages::skill_invoked_in(cycle),
        };
        let _ = runtime.maybe_review(
            signal,
            turn_delivered,
            is_subagent,
            &self.session_id,
            || history.to_vec(),
        );
        // Project notices a *prior* fork queued. The review fork is detached,
        // so a skill learned during turn N typically surfaces at turn N+1's
        // finalize — the same latency as memory's `SystemMemorySavedMessage`.
        self.project_skill_learn_notices(history, event_tx).await;
    }

    /// Dual channel for drained skill notices: a user-visible transcript line
    /// plus a model-visible `<system-reminder>` so the agent knows the skill
    /// exists (and that it is quarantined).
    async fn project_skill_learn_notices(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
    ) {
        let Some(runtime) = self.skill_review_runtime.as_ref() else {
            return;
        };
        for notice in runtime.drain_notices() {
            let user_text = match notice.verb {
                coco_skill_learn::SkillLearnVerb::Learned => format!(
                    "Learned skill: {} — quarantined until 5 successful uses",
                    notice.name
                ),
                coco_skill_learn::SkillLearnVerb::Updated => {
                    format!("Improved skill: {}", notice.name)
                }
            };
            let msg = coco_messages::create_info_message("Skill learning", &user_text);
            crate::history_sync::history_push_and_emit(history, msg, event_tx).await;

            let reminder = match notice.verb {
                coco_skill_learn::SkillLearnVerb::Learned => format!(
                    "A background skill review created the agent skill `{}`. It is \
quarantined: the user can run it with /{}, but you cannot auto-invoke it until it \
proves useful. Mention it if it fits the user's next task.",
                    notice.name, notice.name
                ),
                coco_skill_learn::SkillLearnVerb::Updated => format!(
                    "A background skill review updated the agent skill `{}`.",
                    notice.name
                ),
            };
            let msg = coco_messages::wrapping::create_system_reminder_message_with_kind(
                coco_types::AttachmentKind::SkillLearnedReminder,
                &reminder,
            );
            crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
        }
    }

    /// Build the `FinalizeTurnContext` from engine-side state and
    /// dispatch into `MemoryRuntime::finalize_turn`. The runtime
    /// black-boxes the SM + extract + dream + KAIROS-rollover +
    /// post-write-classify fan-out and returns notices for the engine
    /// to project into history. Subagent gating (`agent_id.is_some()`)
    /// is folded into `is_subagent` rather than a guard at this layer
    /// so the runtime owns the rule.
    pub(crate) async fn build_memory_finalize_ctx_and_run(
        &self,
        history: &MessageHistory,
        estimated_tokens: i64,
        tool_calls_last_turn: i32,
        bare_mode: bool,
        runtime: &Arc<coco_memory::MemoryRuntime>,
    ) -> coco_memory::runtime::FinalizeTurnReport {
        // Pre-compute everything that needs `MessageHistory`. The
        // runtime never re-walks history.
        let last_cursor: Option<String> = runtime.extract.last_cursor().await;
        let sm_cursor: Option<String> = runtime.session_memory.last_extraction_message_id().await;
        let tool_calls_since_sm = count_tool_calls_since(history.as_slice(), sm_cursor.as_deref());
        let last_msg_id = history
            .last()
            .and_then(|m| m.uuid())
            .map(uuid::Uuid::to_string);
        let extract_message_count =
            count_model_visible_since(history.as_slice(), last_cursor.as_deref());

        // Two fresh `messages` clones for the FnOnce closures inside
        // TurnInput. fork_messages and has_memory_writes are evaluated
        // lazily by ExtractService and may fire on the primary OR a
        // trailing stash — both branches need an independent snapshot.
        let messages_for_fork = history.to_vec();
        let messages_for_writes_check = history.to_vec();
        let memory_dir = runtime.personal_dir().to_path_buf();
        let cwd_for_writes_check = self.config.workspace_cwd();
        let last_cursor_for_writes_check = last_cursor.clone();
        let last_cursor_for_fork = last_cursor.clone();

        let extract_input = coco_memory::service::extract::TurnInput {
            fork_messages: Box::new(move || {
                arc_messages_since(&messages_for_fork, last_cursor_for_fork.as_deref())
            }),
            message_count: extract_message_count,
            last_message_id: last_msg_id.clone(),
            has_memory_writes: Box::new(move || {
                main_agent_wrote_memory(
                    &messages_for_writes_check,
                    &memory_dir,
                    &cwd_for_writes_check,
                    last_cursor_for_writes_check.as_deref(),
                )
            }),
        };

        // Gap 4 — direct-edit toast. Walk the just-finished assistant
        // turn for file-mutation calls and pair each with its
        // matching ToolResult so memory's `classify_written_path` pass
        // can decide whether to emit a `ManualEdit` notice.
        let cwd = self.config.workspace_cwd();
        let recent_tool_writes = extract_recent_tool_writes(history.as_slice(), &cwd);

        let ctx = coco_memory::runtime::FinalizeTurnContext {
            estimated_tokens,
            tool_calls_since_sm_cursor: tool_calls_since_sm,
            tool_calls_last_turn,
            last_message_id: last_msg_id,
            auto_compact_enabled: self.config.is_auto_compact_active(),
            bare_mode,
            is_subagent: self.config.agent_id.is_some(),
            now_ms: coco_memory::service::dream::DreamService::now_ms(),
            extract_input,
            recent_tool_writes,
        };

        runtime.finalize_turn(ctx).await
    }
}

pub(super) fn format_memory_update_reminder(
    update: &coco_memory::MemoryUpdateNotice,
    in_context_paths: &[String],
) -> String {
    const MAX_PATHS: usize = 10;

    let source_label = match update.source {
        coco_memory::MemoryUpdateSource::Dream => "Background memory consolidation",
    };
    let mut lines = vec![format!(
        "{source_label} updated your memory directory: {}",
        update.summary
    )];
    if !update.paths.is_empty() {
        lines.push(format!(
            "Files changed: {}",
            format_bounded_path_list(&update.paths, MAX_PATHS)
        ));
    }
    if !in_context_paths.is_empty() {
        lines.push(format!(
            "Your loaded copy of {} is now stale relative to disk - Read it again if you need current contents.",
            format_bounded_path_list(in_context_paths, MAX_PATHS)
        ));
    }
    lines.push(
        "This is ambient context - do not narrate it to the user unless they ask or it is directly relevant to their request."
            .to_string(),
    );
    truncate_memory_reminder(&lines.join("\n"))
}

pub(super) fn format_bounded_path_list(paths: &[String], max_paths: usize) -> String {
    let shown = paths
        .iter()
        .take(max_paths)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let omitted = paths.len().saturating_sub(max_paths);
    if omitted == 0 {
        shown
    } else {
        format!("{shown} ({omitted} more omitted)")
    }
}

pub(super) fn truncate_memory_reminder(text: &str) -> String {
    const MAX_MEMORY_REMINDER_BYTES: usize = 4 * 1024;
    if text.len() <= MAX_MEMORY_REMINDER_BYTES {
        return text.to_string();
    }

    let suffix = format!(
        "\n... omitted {} bytes",
        text.len().saturating_sub(MAX_MEMORY_REMINDER_BYTES)
    );
    let budget = MAX_MEMORY_REMINDER_BYTES.saturating_sub(suffix.len());
    let head = coco_utils_string::take_bytes_at_char_boundary(text, budget);
    format!("{head}{suffix}")
}

/// Walk the last assistant turn for file-mutation tool calls and pair
/// each with its matching `ToolResult` so memory's
/// post-write classification (Gap 4) can decide whether the call
/// produced a `ManualEdit` notice.
///
/// Why only the last assistant turn: notices fire once per turn, so
/// older history was already classified on its own finalize. The
/// matching cost would be `O(history.len())` if we walked the full
/// transcript without buying any extra notices.
///
/// Success is read off `ToolResultMessage.is_error` — the only signal
/// the engine reliably has post-execution. Skipping failed writes
/// means only successful file mutations trigger classification.
///
/// Relative paths are anchored to `cwd` so the downstream
/// `is_within_memory_dir` check (which canonicalises) sees an absolute
/// path, consistent with `main_agent_wrote_memory`.
pub(super) fn extract_recent_tool_writes<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
    cwd: &std::path::Path,
) -> Vec<coco_memory::runtime::ToolWriteRecord> {
    use coco_messages::AssistantContent;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    use std::collections::HashMap;

    let Some(last_assistant_idx) = messages
        .iter()
        .rposition(|m| matches!(m.borrow(), Message::Assistant(_)))
    else {
        return Vec::new();
    };
    let Message::Assistant(last_assistant) = messages[last_assistant_idx].borrow() else {
        return Vec::new();
    };
    let LlmMessage::Assistant { content, .. } = &last_assistant.message else {
        return Vec::new();
    };

    // First pass: collect (tool_call_id, tool_name, file_path) from
    // ToolCall parts that name a write tool with a parseable path.
    // Compare against the typed `ToolName` constants — no raw literals.
    let mut pending: Vec<(String, String, std::path::PathBuf)> = Vec::new();
    for part in content {
        let AssistantContent::ToolCall(tc) = part else {
            continue;
        };
        let name = tc.tool_name.as_str();
        if name == coco_types::ToolName::ApplyPatch.as_str() {
            for path in apply_patch_paths_from_input(&tc.input, cwd) {
                pending.push((tc.tool_call_id.clone(), name.to_string(), path));
            }
            continue;
        }

        let is_write_tool = name == coco_types::ToolName::Write.as_str()
            || name == coco_types::ToolName::Edit.as_str()
            || name == coco_types::ToolName::NotebookEdit.as_str();
        if !is_write_tool {
            continue;
        }
        let Some(file_path_str) = tc
            .input
            .get("file_path")
            .or_else(|| tc.input.get("notebook_path"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let path = std::path::Path::new(file_path_str);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        pending.push((tc.tool_call_id.clone(), name.to_string(), absolute));
    }
    if pending.is_empty() {
        return Vec::new();
    }

    // Index ToolResultMessages after the assistant turn by tool_use_id.
    // Tool results may arrive in any order; build a map then look up.
    let mut results: HashMap<&str, bool> = HashMap::new();
    for msg in &messages[last_assistant_idx + 1..] {
        if let Message::ToolResult(tr) = msg.borrow() {
            results.insert(tr.tool_use_id.as_str(), !tr.is_error);
        }
    }

    pending
        .into_iter()
        .map(
            |(id, tool_name, file_path)| coco_memory::runtime::ToolWriteRecord {
                tool_name,
                file_path,
                // No result yet ⇒ treat as failed; we only emit toasts
                // for confirmed successful writes.
                succeeded: results.get(id.as_str()).copied().unwrap_or(false),
            },
        )
        .collect()
}

pub(super) fn apply_patch_paths_from_input(
    input: &serde_json::Value,
    cwd: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let patch = input
        .get("patch")
        .and_then(|v| v.as_str())
        .or_else(|| input.as_str());
    let Some(patch) = patch else {
        return Vec::new();
    };
    let Ok(cwd) = coco_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd) else {
        return Vec::new();
    };
    let Ok(parsed) = coco_apply_patch::parse_patch(patch) else {
        return Vec::new();
    };
    coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd).permission_paths
}

/// Build a [`crate::prompt_suggestion::SuggestionContext`] from the
/// parent engine's cache slot + app_state snapshot. Used by the
/// pre-fork guards to short-circuit before the API round-trip.
///
/// `assistant_turn_count` and `last_response_was_api_error` come from
/// deserializing the cache slot's `fork_context_messages`;
/// `parent_uncached_tokens` is the last assistant's
/// `input - cache_read_input + output` tokens. Other fields come from
/// `ToolAppState`.
pub(super) async fn build_suggestion_context(
    cache: &coco_types::CacheSafeParams,
    app_state: &std::sync::Arc<tokio::sync::RwLock<coco_types::ToolAppState>>,
    is_non_interactive: bool,
    is_teammate: bool,
) -> crate::prompt_suggestion::SuggestionContext {
    let mut assistant_turn_count: u32 = 0;
    let mut last_assistant_msg: Option<&coco_messages::AssistantMessage> = None;
    for arc in &cache.fork_context_messages {
        if let coco_messages::Message::Assistant(a) = arc.as_ref() {
            assistant_turn_count = assistant_turn_count.saturating_add(1);
            last_assistant_msg = Some(a);
        }
    }

    let (last_response_was_api_error, parent_uncached_tokens) = match last_assistant_msg {
        Some(a) => {
            let api_error = a.api_error.is_some();
            let usage = a.usage.unwrap_or_default();
            let tokens = crate::prompt_suggestion::parent_uncached_tokens(&usage);
            (api_error, tokens)
        }
        None => (false, 0),
    };

    let snap = app_state.read().await;
    let plan_mode = matches!(
        snap.permissions.mode,
        Some(coco_types::PermissionMode::Plan)
    );
    let awaiting_plan_approval = snap.awaiting_plan_approval;
    // Phase 7 wire-up: read live counters from `ToolAppState`. Both
    // counters are `Arc<AtomicU32>`, mutated lock-free by RAII guards
    // held by the TUI permission bridge (`pending_permission_count`)
    // and the MCP elicitation service (`elicitation_pending_count`).
    let pending_permission = snap
        .pending_permission_count
        .load(std::sync::atomic::Ordering::Relaxed)
        > 0;
    let elicitation_active = snap
        .elicitation_pending_count
        .load(std::sync::atomic::Ordering::Relaxed)
        > 0;
    // Phase 7c: selective rate-limit suppression — `rate_limits` is
    // keyed by provider instance name; we look up the cache's
    // recorded provider so fast-mode swaps are honoured (the parent
    // turn captured the literally-active provider).
    let now_ms = chrono::Utc::now().timestamp_millis();
    let rate_limit = if cache.provider.is_empty() {
        // Pre-Phase-7 transcripts may carry empty `provider` (serde
        // default). Without a key we can't match selectively; fail
        // open (no suppression) to avoid silencing all suggestions.
        false
    } else {
        snap.rate_limits
            .get(&cache.provider)
            .map(|e| {
                matches!(e.status, coco_types::RateLimitStatus::Rejected)
                    && e.reset_at_ms.is_none_or(|r| now_ms < r)
            })
            .unwrap_or(false)
    };
    let env_disable =
        coco_config::env::is_env_truthy(coco_config::EnvKey::CocoPromptSuggestionDisable);
    let bare_mode = coco_config::env::is_env_truthy(coco_config::EnvKey::CocoBareMode);
    drop(snap);

    crate::prompt_suggestion::SuggestionContext {
        assistant_turn_count,
        last_response_was_api_error,
        parent_uncached_tokens,
        disabled: env_disable,
        pending_permission,
        is_teammate,
        awaiting_plan_approval,
        elicitation_active,
        plan_mode,
        rate_limit,
        bare_mode,
        non_interactive: is_non_interactive,
    }
}

/// Slice the message history to "everything newer than `last_cursor`"
/// for `AgentSpawnRequest::fork_context_messages`. When `last_cursor`
/// is `None` (first extraction), return the full history.
///
/// Takes the engine's already-shared `Arc<Message>` slice and
/// `Arc::clone`s each entry — no deep `Message` body clones at
/// this seam.
pub(super) fn arc_messages_since(
    messages: &[std::sync::Arc<coco_messages::Message>],
    last_cursor: Option<&str>,
) -> Vec<std::sync::Arc<coco_messages::Message>> {
    let cursor_idx = last_cursor.and_then(|c| {
        messages
            .iter()
            .position(|m| m.uuid().map(|u| u.to_string() == c).unwrap_or(false))
    });
    let slice = match cursor_idx {
        Some(i) => &messages[i + 1..],
        None => messages,
    };
    slice.to_vec()
}

/// Count user + assistant messages strictly after `since_uuid`.
/// "Model-visible" = anything sent in API calls; excludes progress,
/// system, attachment, tombstone, tool_use_summary. Threaded into
/// the extraction agent's prompt so the "~N messages" guidance is
/// accurate (using `history.len()` would over-count).
///
/// Fall-through: when `since_uuid` is `None` or doesn't match any
/// message in `messages` (e.g. compaction trimmed the cursor), count
/// the whole history so a stale cursor doesn't permanently zero the
/// count.
pub(super) fn count_model_visible_since<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
    since_uuid: Option<&str>,
) -> i32 {
    use coco_messages::Message;
    let is_visible = |m: &Message| matches!(m, Message::User(_) | Message::Assistant(_));
    let cursor_idx = since_uuid.and_then(|c| {
        messages.iter().position(|m| {
            m.borrow()
                .uuid()
                .map(|u| u.to_string() == c)
                .unwrap_or(false)
        })
    });
    let start = match cursor_idx {
        Some(i) => i + 1,
        None => 0,
    };
    messages[start..]
        .iter()
        .filter(|m| is_visible(m.borrow()))
        .count() as i32
}

/// Count cumulative `tool_use` blocks across all assistant messages
/// strictly after `since_uuid` (or all messages when the cursor is
/// `None` / not found). This is the gate signal `SessionMemoryService`
/// uses to decide if enough work has accumulated since the last
/// extraction.
pub(super) fn count_tool_calls_since<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
    since_uuid: Option<&str>,
) -> i32 {
    use coco_messages::AssistantContent;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    let cursor_idx = since_uuid.and_then(|c| {
        messages.iter().position(|m| {
            m.borrow()
                .uuid()
                .map(|u| u.to_string() == c)
                .unwrap_or(false)
        })
    });
    let start = match cursor_idx {
        Some(i) => i + 1,
        None => 0,
    };
    let mut count: i32 = 0;
    for msg in &messages[start..] {
        if let Message::Assistant(assistant) = msg.borrow()
            && let LlmMessage::Assistant { content, .. } = &assistant.message
        {
            for block in content {
                if matches!(block, AssistantContent::ToolCall(_)) {
                    count = count.saturating_add(1);
                }
            }
        }
    }
    count
}

/// Detect whether any assistant turn since `since_uuid` wrote into the
/// memory directory via a file-mutation tool. Used by
/// `ExtractService::maybe_extract` to skip extraction when the user
/// just curated memory directly. When `since_uuid` is `None` (or
/// the cursor uuid isn't found, e.g. compaction trimmed it), walk the
/// entire history so a stale cursor doesn't permanently mask writes.
pub(super) fn main_agent_wrote_memory<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
    memory_dir: &std::path::Path,
    cwd: &std::path::Path,
    since_uuid: Option<&str>,
) -> bool {
    use coco_messages::AssistantContent;
    use coco_messages::LlmMessage;
    use coco_messages::Message;
    let cursor_idx = since_uuid.and_then(|c| {
        messages.iter().position(|m| {
            m.borrow()
                .uuid()
                .map(|u| u.to_string() == c)
                .unwrap_or(false)
        })
    });
    let start = match cursor_idx {
        Some(i) => i + 1,
        None => 0,
    };
    for msg in &messages[start..] {
        let Message::Assistant(assistant) = msg.borrow() else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        for block in content {
            let AssistantContent::ToolCall(call) = block else {
                continue;
            };
            // Compare against the canonical typed names instead of
            // raw string literals.
            let name = call.tool_name.as_str();
            if name == coco_types::ToolName::ApplyPatch.as_str() {
                if apply_patch_paths_from_input(&call.input, cwd)
                    .iter()
                    .any(|path| coco_memory::path::is_auto_mem_file(path, memory_dir))
                {
                    return true;
                }
                continue;
            }

            let is_write_tool = name == coco_types::ToolName::Write.as_str()
                || name == coco_types::ToolName::Edit.as_str()
                || name == coco_types::ToolName::NotebookEdit.as_str();
            if !is_write_tool {
                continue;
            }
            let Some(file_path) = call
                .input
                .get("file_path")
                .or_else(|| call.input.get("notebook_path"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let path = std::path::Path::new(file_path);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            if coco_memory::path::is_auto_mem_file(&absolute, memory_dir) {
                return true;
            }
        }
    }
    false
}

/// Phase 7c: prune `rate_limits` entries whose `reset_at_ms` has
/// passed. Called from `finalize_turn_post_tools` immediately before
/// `spawn_prompt_suggestion_task` reads the map. Bounded keyspace
/// (≤ #configured providers) means pruning is O(few entries) per
/// finalize and there's no hot-path concern.
///
/// Entries with `reset_at_ms = None` (no reset header surfaced) are
/// retained — they get overwritten on the next successful or failing
/// call from the same provider. Bounded by the keyspace anyway.
pub(super) async fn prune_stale_rate_limits(
    app_state: &std::sync::Arc<tokio::sync::RwLock<coco_types::ToolAppState>>,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut snap = app_state.write().await;
    snap.rate_limits
        .retain(|_, e| e.reset_at_ms.is_none_or(|r| r > now_ms));
}
