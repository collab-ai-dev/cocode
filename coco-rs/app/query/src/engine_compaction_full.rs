use super::*;

impl QueryEngine {
    /// Attempt full LLM-summarized compaction.
    ///
    /// Snapshots readFileState, clears it, calls the LLM to summarize
    /// old rounds, then re-injects recently read files.
    ///
    /// Sequence:
    /// 1. SM-first short-circuit (Auto path only — manual handled in
    ///    `run_manual_compact`). Returns immediately if SM produced a result.
    /// 2. PreCompact hooks — collect any custom instructions and merge
    ///    into the summary prompt.
    /// 3. Snapshot FileReadState; clear it only after summary success.
    /// 4. Call `compact_conversation` with the LLM summarizer.
    /// 5. Notify CompactionObservers.
    /// 6. PostCompact hooks.
    #[tracing::instrument(
        skip_all,
        name = "compaction",
        fields(
            trigger = ?trigger,
            session_id = %self.session_id,
            history_len = history.len(),
            has_custom_instructions = custom_instructions.is_some(),
        ),
    )]
    pub(crate) async fn try_full_compact(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        trigger: coco_types::CompactTrigger,
        custom_instructions: Option<String>,
    ) -> coco_compact::CompactOutcome {
        self.try_full_compact_impl(history, event_tx, trigger, custom_instructions, None)
            .await
    }

    pub(super) async fn try_full_compact_impl(
        &self,
        history: &mut MessageHistory,
        event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
        trigger: coco_types::CompactTrigger,
        custom_instructions: Option<String>,
        manual_request: Option<&ManualCompactRequest>,
    ) -> coco_compact::CompactOutcome {
        let trigger_label = match trigger {
            coco_types::CompactTrigger::Manual => "manual",
            coco_types::CompactTrigger::Auto => "auto",
            coco_types::CompactTrigger::Reactive => "reactive",
            coco_types::CompactTrigger::TimeBased => "time_based",
            coco_types::CompactTrigger::SessionMemory => "session_memory",
            coco_types::CompactTrigger::ContextCollapse => "context_collapse",
        };
        info!(trigger = trigger_label, "try_full_compact entered");
        // Hook wire trigger is `enum('manual','auto')`. Runtime-only
        // triggers (Reactive / TimeBased / SessionMemory /
        // ContextCollapse) all map to `Auto` for the hook payload —
        // they are autonomous compaction events from the agent's
        // perspective.
        let hook_trigger = match trigger {
            coco_types::CompactTrigger::Manual => coco_hooks::orchestration::CompactTrigger::Manual,
            _ => coco_hooks::orchestration::CompactTrigger::Auto,
        };

        // 1. SM-first short-circuit. Auto always tries SM; Manual tries
        //    SM only when the user gave no custom instructions — with
        //    instructions the user wants the LLM to honor them, and SM
        //    can't.
        let can_try_sm = match trigger {
            coco_types::CompactTrigger::Auto => true,
            coco_types::CompactTrigger::Manual => custom_instructions.is_none(),
            _ => false,
        };
        if can_try_sm
            && self.config.compact.session_memory.enabled
            && self
                .try_session_memory_compact(history, event_tx, trigger, manual_request)
                .await
        {
            return coco_compact::CompactOutcome::Applied;
        }

        // Emit phase: HooksStart{PreCompact} — drives the "Running
        // PreCompact hooks…" spinner.
        let _ = emit_protocol(
            event_tx,
            ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::HooksStart,
                hook_type: Some(coco_types::CompactionHookType::PreCompact),
            }),
        )
        .await;

        // 1. PreCompact hooks. They may produce additional custom_instructions
        //    that get merged into the summary prompt, plus a userDisplayMessage
        //    for the TUI.
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
                Err(e) => warn!("PreCompact hook execution failed: {e}"),
            }
        }

        // 2. Snapshot FileReadState. Clear it only after summary success
        // so a failed compact attempt leaves read-file dedup state intact.
        let snapshot = if let Some(frs) = &self.file_read_state {
            let frs = frs.read().await;
            frs.snapshot_by_recency()
        } else {
            Vec::new()
        };

        // 2. Build the attachment callback that captures the snapshot.
        // Post-compact attachment assembly: file attachments, plan
        // attachment, plan-mode attachment, async-agent attachments,
        // and in-band skill re-injection.
        let cwd = self.config.workspace_cwd();
        let session_id = self.session_id.clone();
        let config_home = self.config_home.clone();
        let project_dir = self.config.project_dir.clone();
        let plans_directory_setting = self.config.plans_directory.clone();
        let captured_skills: Vec<coco_compact::PostCompactSkill> = self
            .post_compact_skills
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        // Plan-mode snapshot: read live permission mode from `ToolAppState`;
        // workflow / phase4_variant / agent counts come from QueryEngineConfig.
        let agent_id_for_attachments = self.config.agent_id.clone();
        let captured_plan_mode_snapshot: Option<coco_compact::PlanModeAttachment> = {
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
                None
            } else {
                let (loaded_tools, deferred_tools) = self
                    .current_tool_search_partitions(&app_state_snapshot)
                    .await;
                let pm = &self.config.plan_mode_settings;
                let workflow = match pm.workflow {
                    coco_config::PlanModeWorkflow::FivePhase => {
                        coco_context::PlanWorkflow::FivePhase
                    }
                    coco_config::PlanModeWorkflow::Interview => {
                        coco_context::PlanWorkflow::Interview
                    }
                };
                let phase4 = match pm.phase4_variant {
                    coco_config::PlanPhase4Variant::Standard => {
                        coco_context::Phase4Variant::Standard
                    }
                    coco_config::PlanPhase4Variant::Trim => coco_context::Phase4Variant::Trim,
                    coco_config::PlanPhase4Variant::Cut => coco_context::Phase4Variant::Cut,
                    coco_config::PlanPhase4Variant::Cap => coco_context::Phase4Variant::Cap,
                };
                let (plan_path, plan_exists_flag) =
                    match (config_home.as_deref(), session_id.as_str()) {
                        (Some(ch), sid) if !sid.is_empty() => {
                            let plans_dir = coco_context::resolve_plans_directory(
                                ch,
                                project_dir.as_deref(),
                                plans_directory_setting.as_deref(),
                            );
                            let path = coco_context::get_plan_file_path(
                                sid,
                                &plans_dir,
                                agent_id_for_attachments
                                    .as_ref()
                                    .map(coco_types::AgentId::as_str),
                            );
                            let exists = coco_context::plan_exists(
                                sid,
                                &plans_dir,
                                agent_id_for_attachments
                                    .as_ref()
                                    .map(coco_types::AgentId::as_str),
                            );
                            (path.display().to_string(), exists)
                        }
                        _ => (String::new(), false),
                    };
                Some(coco_compact::PlanModeAttachment {
                    reminder_type: coco_context::ReminderType::Full,
                    workflow,
                    custom_instructions: pm.custom_instructions.clone(),
                    phase4_variant: phase4,
                    explore_agent_count: pm.explore_agent_count,
                    plan_agent_count: pm.plan_agent_count,
                    explore_plan_agents_available: self
                        .explore_plan_agents_available(&loaded_tools),
                    is_sub_agent: agent_id_for_attachments.is_some(),
                    plan_file_path: plan_path,
                    plan_exists: plan_exists_flag,
                    // Model-aware plan-file tool (gpt-5 → apply_patch, Claude → Write/Edit).
                    write_tool: self.config.tool_overrides.write_tool(),
                    edit_tool: self.config.tool_overrides.edit_tool(),
                    deferred_tools,
                })
            }
        };
        // Snapshot recently @mentioned paths for priority restoration.
        // The closure runs synchronously inside `compact_conversation`, so
        // we read the lock now and move the resolved set in.
        let prioritized_paths = self.recently_mentioned_paths_snapshot().await;
        let max_files_to_restore =
            self.config.compact.post_compact.max_files_to_restore.max(0) as usize;
        let attachment_fn: coco_compact::compact::PostCompactAttachmentFn =
            Box::new(move |result: &coco_compact::CompactResult| {
                // Resolve plan file path for exclusion from file restore.
                let plan_file = config_home.as_ref().map(|ch| {
                    let plans_dir = coco_context::resolve_plans_directory(
                        ch,
                        project_dir.as_deref(),
                        plans_directory_setting.as_deref(),
                    );
                    coco_context::get_plan_file_path(
                        session_id.as_str(),
                        &plans_dir,
                        agent_id_for_attachments
                            .as_ref()
                            .map(coco_types::AgentId::as_str),
                    )
                });

                let mut atts =
                    coco_compact::create_post_compact_file_attachments_with_priority_and_limit(
                        &snapshot,
                        &result.messages_to_keep,
                        &cwd,
                        plan_file.as_deref(),
                        &prioritized_paths,
                        max_files_to_restore,
                    );

                // Re-inject the plan file's content so it survives
                // the compaction boundary.
                if let Some(ref ch) = config_home {
                    let plans_dir = coco_context::resolve_plans_directory(
                        ch,
                        project_dir.as_deref(),
                        plans_directory_setting.as_deref(),
                    );
                    let plan_path = coco_context::get_plan_file_path(
                        session_id.as_str(),
                        &plans_dir,
                        agent_id_for_attachments
                            .as_ref()
                            .map(coco_types::AgentId::as_str),
                    );
                    let plan_content = coco_context::get_plan(
                        session_id.as_str(),
                        &plans_dir,
                        agent_id_for_attachments
                            .as_ref()
                            .map(coco_types::AgentId::as_str),
                    );
                    if let Some(att) = coco_compact::create_plan_attachment_if_needed(
                        &plan_path,
                        plan_content.as_deref(),
                    ) {
                        atts.push(att);
                    }
                }

                // In-band skill re-injection: each invoked skill surfaces
                // both as a post-compact attachment AND in the next-turn
                // `<system-reminder>` (double-write for budget resilience).
                atts.extend(coco_compact::create_post_compact_skill_attachments(
                    &captured_skills,
                ));

                // When in plan mode at compact time, re-emit `plan_mode`
                // reminderType='full' so plan instructions land on the
                // FIRST post-compact turn rather than waiting for the
                // system-reminder cadence to next fire.
                if let Some(pm_attachment) = captured_plan_mode_snapshot
                    && let Some(att) = coco_compact::create_plan_mode_attachment_if_needed(
                        /*is_plan_mode*/ true,
                        pm_attachment,
                    )
                {
                    atts.push(att);
                }

                atts
            });

        // Emit phase: Summarizing — TUI flips spinner to "Compacting conversation".
        let _ = emit_protocol(
            event_tx,
            ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::Summarizing,
                hook_type: None,
            }),
        )
        .await;

        // 3. Build compact run-options. `custom_prompt` carries any
        //    instructions returned by PreCompact hooks merged with the
        //    user's `/compact <instructions>` argument.
        // Derive RecompactionInfo from the last-compact tracker.
        // Auto-compact threshold mirrors the gate we already evaluated
        // above, recomputed here for the analytics-aligned struct.
        let auto_threshold = coco_compact::auto_compact_threshold(
            self.resolved_context_window(),
            self.resolved_max_output_tokens(),
            &self.config.compact.auto,
        );
        // `RecompactionInfo.turns_since_previous`: counter was reset to
        // 0 on the previous compact and bumped per turn since.
        let recompaction_info = self
            .last_compact_state
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .map(|prev| coco_compact::types::RecompactionInfo {
                is_recompaction: true,
                turns_since_previous: prev.turn_counter as i32,
                auto_compact_threshold: auto_threshold,
            });
        let compact_run_options = coco_compact::CompactRunOptions {
            context_window: self.resolved_context_window(),
            trigger,
            custom_prompt: effective_instructions,
            recompaction_info,
            ..Default::default()
        };

        // 4. Call compact_conversation with the query-level summary executor.
        // It prefers a cache-sharing compact fork and falls back to a
        // no-tools structured direct call when no dispatcher is installed.
        let summarize_fn = |attempt: coco_compact::CompactSummaryAttempt| async move {
            self.run_compact_summary_attempt(attempt, event_tx).await
        };

        match coco_compact::compact_conversation(
            history.as_slice(),
            &compact_run_options,
            summarize_fn,
            Some(attachment_fn),
        )
        .await
        {
            Ok(mut result) => {
                if result.summary_messages.is_empty() {
                    if let Some(request) = manual_request {
                        append_manual_compact_notice(
                            history,
                            event_tx,
                            request,
                            "Not enough messages to compact.",
                        )
                        .await;
                    }
                    let _ = emit_protocol(
                        event_tx,
                        ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
                            phase: coco_types::CompactionPhase::Done,
                            hook_type: None,
                        }),
                    )
                    .await;
                    return coco_compact::CompactOutcome::Skipped;
                }

                info!(
                    pre = result.pre_compact_tokens,
                    post = result.post_compact_tokens,
                    "full compaction completed (trigger={trigger_label})"
                );

                // Carry any PreCompact userDisplayMessage forward so the
                // TUI can show it next to the boundary marker.
                if let Some(msg) = pre_display.as_ref() {
                    result.user_display_message = Some(match result.user_display_message {
                        Some(prev) => format!("{prev}\n{msg}"),
                        None => msg.clone(),
                    });
                }

                // PostCompact hooks receive the raw LLM summary before
                // it is wrapped in continuation boilerplate.
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
                        Err(e) => warn!("PostCompact hook execution failed: {e}"),
                    }
                }

                // Run SessionStart hooks after the LLM-summarized path.
                // Render those hook events into the rewritten history
                // directly so they are not also emitted by the next-turn
                // sync reminder buffer.
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
                        self.compact_session_start_hook_messages(registry, "compact")
                            .await,
                    );
                }

                if let Some(request) = manual_request {
                    append_manual_compact_breadcrumbs(&mut result, request);
                }

                let (delta_attachments, delta_state) = self
                    .create_post_compact_delta_attachments::<std::sync::Arc<Message>>(&[])
                    .await;
                result.attachments.extend(delta_attachments);

                // Canonical message order: boundary, summaryMessages,
                // messagesToKeep, attachments, hookResults.
                let summary_tokens = result.post_compact_tokens as i32;
                let new_messages = coco_compact::build_post_compact_messages(&result);
                let pre_len = history.len() as i32;
                let post_len = new_messages.len() as i32;
                let removed_messages = (pre_len - post_len).max(0);
                // I-1 (Authority): full LLM compaction rewrites the
                // engine-authoritative history. Pair the swap with a
                // `MessageTruncated { 0 }` + per-message
                // `MessageAppended` burst so the TUI/SDK derived views
                // track the new state.
                self.replace_history_after_compact(history, new_messages.clone(), event_tx)
                    .await;
                self.update_post_compact_delta_state(delta_state).await;

                if let Some(frs) = &self.file_read_state {
                    let mut frs = frs.write().await;
                    frs.clear();
                }

                // Record the successful compaction for the next turn's
                // `RecompactionInfo`: reset `turn_counter = 0` and
                // freshen the run_id.
                let run_id = result.boundary_marker.uuid().copied().unwrap_or_default();
                tracing::info!(
                    target: "coco_query::compact_track",
                    run_id = %run_id,
                    trigger = ?trigger,
                    "autocompact boundary recorded (turn_counter reset to 0)"
                );
                if let Ok(mut guard) = self.last_compact_state.lock() {
                    *guard = Some(crate::engine::LastCompactState {
                        turn_counter: 0,
                        run_id: run_id.to_string(),
                    });
                }

                // Notify each registered observer so per-crate caches
                // (file/memory/skill state) drop their pre-compact entries.
                // `is_main_agent = agent_id.is_none()`: subagent
                // compactions must not wipe main-thread state.
                let is_main_agent = self.config.agent_id.is_none();
                self.compaction_observers
                    .notify_all(&result, is_main_agent)
                    .await;
                self.compaction_observers
                    .notify_post_compact(&new_messages)
                    .await;
                // After full LLM compaction the message list is rewritten;
                // reset the cache-break baseline so the new baseline is
                // not compared against pre-compact cache_read tokens.
                let qs = self.query_source_label();
                self.notify_model_compaction(qs).await;
                // Full LLM-summarized compact path — same reasoning as
                // the SM-first and partial paths: relevant-memory
                // attachments are dropped from history, so the recall
                // state's dedup set and 60 KB byte budget must reset
                // to match. ALSO clear the SM in-memory cache — the
                // pre-compact content is no longer the right baseline
                // for the next SM-first short-circuit attempt.
                if let Some(rt) = &self.memory_runtime {
                    rt.reset_recall_state();
                    rt.session_memory.clear_after_compact().await;
                }

                let _delivered = emit_protocol(
                    event_tx,
                    ServerNotification::ContextCompacted(coco_types::ContextCompactedParams {
                        removed_messages,
                        summary_tokens,
                        trigger,
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
                // Surface task_status reminders on the next turn.
                self.pending_just_compacted
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                coco_compact::CompactOutcome::Applied
            }
            Err(e) => {
                warn!("full compaction failed: {e}");
                if manual_request.is_some() {
                    emit_manual_compaction_failed(
                        event_tx,
                        &format!("Error during compaction: {e}"),
                    )
                    .await;
                }
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
}

pub(super) struct PostCompactDeltaState {
    pub(super) current_deferred_tools: Vec<String>,
    pub(super) agent_id: Option<String>,
    pub(super) current_agents: Vec<String>,
    pub(super) current_mcp_instructions: HashMap<String, String>,
    pub(super) current_mcp_servers:
        std::collections::BTreeMap<String, coco_types::McpServerAnnouncementState>,
}

pub(super) fn preserved_contains_attachment_kind<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    kind: coco_types::AttachmentKind,
) -> bool {
    messages
        .iter()
        .any(|m| matches!(m.borrow(), Message::Attachment(att) if att.kind == kind))
}

pub(super) async fn emit_manual_compaction_failed(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    error: &str,
) {
    let _ = emit_protocol(
        event_tx,
        ServerNotification::CompactionFailed(coco_types::CompactionFailedParams {
            error: error.to_string(),
            attempts: 1,
        }),
    )
    .await;
}

pub(super) async fn emit_compaction_done(event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>) {
    let _ = emit_protocol(
        event_tx,
        ServerNotification::CompactionPhase(coco_types::CompactionPhaseParams {
            phase: coco_types::CompactionPhase::Done,
            hook_type: None,
        }),
    )
    .await;
}

pub(super) async fn append_manual_compact_notice(
    history: &mut MessageHistory,
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    request: &ManualCompactRequest,
    notice: &str,
) {
    for msg in coco_messages::build_slash_command_messages(
        "compact",
        &request.command_args,
        notice,
        /*is_sensitive*/ false,
    ) {
        crate::history_sync::history_push_and_emit(history, msg, event_tx).await;
    }
}

pub(super) fn append_manual_compact_breadcrumbs(
    result: &mut coco_compact::CompactResult,
    request: &ManualCompactRequest,
) {
    let breadcrumbs = [
        Arc::new(Message::Attachment(AttachmentMessage::api(
            coco_types::AttachmentKind::CriticalSystemReminder,
            LlmMessage::user_text(
                "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.</local-command-caveat>",
            ),
        ))),
        Arc::new(create_manual_compact_user_message(
            &coco_messages::format_command_input("compact", &request.command_args),
        )),
        Arc::new(create_manual_compact_user_message(
            &coco_messages::format_local_command_stdout(&manual_compact_stdout(result)),
        )),
    ];
    result.messages_to_keep.extend(breadcrumbs);
}

pub(super) fn create_manual_compact_user_message(content: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(content),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::SlashCommand),
        parent_tool_use_id: None,
    })
}

pub(super) fn manual_compact_stdout(result: &coco_compact::CompactResult) -> String {
    let mut text = if result.pre_compact_tokens > 0 && result.post_compact_tokens > 0 {
        let saved = result
            .pre_compact_tokens
            .saturating_sub(result.post_compact_tokens);
        let saved_percent = (saved as f64 / result.pre_compact_tokens as f64) * 100.0;
        format!(
            "Compacted ({} -> {} tokens, saved {} / {:.1}%; Ctrl+O to see full summary)",
            result.pre_compact_tokens, result.post_compact_tokens, saved, saved_percent
        )
    } else {
        "Compacted (Ctrl+O to see full summary)".to_string()
    };
    if let Some(message) = result.user_display_message.as_deref()
        && !message.trim().is_empty()
    {
        text.push('\n');
        text.push_str(message);
    }
    text
}

pub(super) fn extract_compact_summary_from_messages(
    messages: &[std::sync::Arc<Message>],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err(format!("{COMPACT_SUMMARY_ABORTED_PREFIX} cancelled"));
    }

    let mut chunks = Vec::new();
    for message in messages {
        let Message::Assistant(assistant) = message.as_ref() else {
            continue;
        };
        if let Some(api_error) = &assistant.api_error {
            return Err(format!(
                "{COMPACT_SUMMARY_INVALID_PREFIX} assistant API error: {}",
                api_error.message
            ));
        }
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        chunks.push(extract_compact_summary_from_content(content)?);
    }

    Ok(chunks.join("\n"))
}

/// One compaction-attributed model-fallback record. The runtime's
/// `FallbackSwitched` event already carries the `ModelRuntimeSource`
/// (`Role(Main)` for compaction), but that source is shared with the main
/// loop — the `query_source` label is what makes a *compaction* fallback
/// distinguishable in telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompactFallbackRecord<'a> {
    pub(super) query_source: &'a str,
    pub(super) from_model_id: &'a str,
    pub(super) to_model_id: &'a str,
}

/// Project the runtime events emitted during a summarize call into the
/// compaction-attributed fallback records (only `FallbackSwitched` carries
/// a model switch worth attributing). Pure so the attribution is testable
/// without exercising the live runtime.
pub(super) fn compact_fallback_records<'a>(
    query_source: &'a str,
    events: &'a [coco_inference::ModelRuntimeEvent],
) -> Vec<CompactFallbackRecord<'a>> {
    events
        .iter()
        .filter_map(|event| match event {
            coco_inference::ModelRuntimeEvent::FallbackSwitched {
                from_model_id,
                to_model_id,
                ..
            } => Some(CompactFallbackRecord {
                query_source,
                from_model_id: from_model_id.as_str(),
                to_model_id: to_model_id.as_str(),
            }),
            _ => None,
        })
        .collect()
}

/// Emit compaction-source attribution for any model fallback that fired
/// during a summarize call, using the standard `query_source` field name
/// (see `common/otel/CLAUDE.md`).
pub(super) fn log_compact_fallback_events(
    query_source: &str,
    events: &[coco_inference::ModelRuntimeEvent],
) {
    for record in compact_fallback_records(query_source, events) {
        warn!(
            query_source = record.query_source,
            from_model_id = record.from_model_id,
            to_model_id = record.to_model_id,
            "model fallback triggered during compaction summarize"
        );
    }
}

pub(super) fn compact_summary_query_params(
    prompt: Vec<LlmMessage>,
    max_summary_tokens: i64,
    fallback_min_context_window: Option<i64>,
    cache_scope: Option<&coco_types::SessionId>,
) -> QueryParams {
    QueryParams {
        prompt,
        temperature: None,
        max_tokens: Some(max_summary_tokens),
        // Multi-provider compact helper: do not force an explicit
        // "disable thinking" override here. Some providers/models have
        // no supported off-toggle, and `None` lets inference apply the
        // resolved model default without emitting unsupported provider
        // options.
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some(COMPACT_QUERY_SOURCE.to_string()),
        agent_id: None,
        cache_scope: cache_scope.map(|session_id| session_id.as_str().to_string()),
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        fallback_min_context_window,
        cancel: None,
        wire_tap: None,
    }
}

pub(super) fn extract_compact_summary_from_content(
    content: &[AssistantContent],
) -> Result<String, String> {
    let mut chunks = Vec::new();
    for c in content {
        match c {
            AssistantContent::Text(t) if !t.text.is_empty() => chunks.push(t.text.clone()),
            AssistantContent::ToolCall(tc) => {
                return Err(format!(
                    "{COMPACT_SUMMARY_INVALID_PREFIX} summary attempted tool call {}",
                    tc.tool_name
                ));
            }
            _ => {}
        }
    }
    Ok(chunks.join("\n"))
}
