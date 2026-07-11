pub(super) async fn emit_resume_plan_ui_state(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    restored_v1_todos: Option<Vec<coco_types::TodoRecord>>,
) {
    let cfg = session.current_engine_config().await;
    let goal = goal_command::restore_goal_from_history(
        &plan
            .prior_messages
            .iter()
            .cloned()
            .map(std::sync::Arc::new)
            .collect::<Vec<_>>(),
        session.app_state(),
        &session.hook_registry(),
        session.session_usage_snapshot().await.totals.output_tokens,
        goal_command::GoalGate {
            hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
            trust_rejected: workspace_trust_rejected(),
        },
    )
    .await;

    // Bulk resume hydration mirrors the startup `--resume` path:
    // reset UI-only state first, then replace transcript scrollback in
    // one pass instead of replaying thousands of individual appends.
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionResetForResume {
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
            },
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::HistoryReplaced {
                messages: plan
                    .prior_messages
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect(),
                identity: coco_types::ServerNotificationIdentity::new(
                    Some(plan.session_id.clone()),
                    None,
                ),
                reason: coco_types::HistoryReplaceReason::Hydrate,
            },
        ))
        .await;
    if let Some(todos) = restored_v1_todos {
        let mut todos_by_agent = HashMap::new();
        if !todos.is_empty() {
            todos_by_agent.insert(plan.session_id.to_string(), todos);
        }
        let _ = event_tx
            .send(CoreEvent::Protocol(
                coco_types::ServerNotification::TaskPanelChanged(
                    coco_types::TaskPanelChangedParams {
                        plan_tasks: Vec::new(),
                        todos_by_agent,
                        expanded_view: coco_types::ExpandedView::None,
                        verification_nudge_pending: false,
                        // Unordered producer: always applied, never
                        // advances the consumer's high-water mark.
                        generation: 0,
                    },
                ),
            ))
            .await;
    }
    let _ = event_tx
        .send(CoreEvent::Protocol(
            coco_types::ServerNotification::SessionUsageUpdated(Box::new(
                session.session_usage_snapshot().await,
            )),
        ))
        .await;
    let _ = event_tx
        .send(CoreEvent::Protocol(
            goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
    session
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

pub(super) async fn emit_resume_plan_ui_state_for_runtime(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let restored_v1_todos = if session
        .runtime_config()
        .features
        .enabled(coco_types::Feature::TaskV2)
    {
        None
    } else {
        latest_todo_write_todos(&plan.prior_messages)
    };
    if let Some(todos) = restored_v1_todos.clone() {
        session
            .seed_todo_list_snapshot(plan.session_id.to_string(), todos)
            .await;
    }
    emit_resume_plan_ui_state(plan, session, event_tx, restored_v1_todos).await;
}

#[derive(serde::Deserialize)]
pub(super) struct TodoWriteTranscriptInput {
    todos: Vec<coco_types::TodoRecord>,
}

pub(super) fn todo_write_store_snapshot(
    todos: Vec<coco_types::TodoRecord>,
) -> Vec<coco_types::TodoRecord> {
    if !todos.is_empty() && todos.iter().all(|todo| todo.status == "completed") {
        Vec::new()
    } else {
        todos
    }
}

pub(super) fn latest_todo_write_todos(messages: &[Message]) -> Option<Vec<coco_types::TodoRecord>> {
    for message in messages.iter().rev() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let LlmMessage::Assistant { content, .. } = &assistant.message else {
            continue;
        };
        for part in content.iter().rev() {
            let AssistantContent::ToolCall(call) = part else {
                continue;
            };
            if call.tool_name != coco_types::ToolName::TodoWrite.as_str() {
                continue;
            }
            match serde_json::from_value::<TodoWriteTranscriptInput>(call.input.clone()) {
                Ok(input) => return Some(todo_write_store_snapshot(input.todos)),
                Err(err) => {
                    warn!(error = %err, "failed to restore TodoWrite state from transcript input");
                    return None;
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_resume(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let target = args.trim();
    if target.is_empty() {
        let manager = Arc::clone(runtime.session_manager());
        let sessions = match tokio::task::spawn_blocking(move || manager.list()).await {
            Ok(Ok(sessions)) => sessions,
            Ok(Err(err)) => {
                emit_slash_text(
                    event_tx,
                    "resume",
                    args,
                    &format!("Failed to list sessions: {err}"),
                )
                .await;
                return SlashOutcome::Handled;
            }
            Err(err) => {
                emit_slash_text(
                    event_tx,
                    "resume",
                    args,
                    &format!("Session listing task failed: {err}"),
                )
                .await;
                return SlashOutcome::Handled;
            }
        };
        let sessions = sessions
            .into_iter()
            .filter_map(session_to_sdk_summary)
            .collect();
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenSessionBrowser {
                sessions,
            }))
            .await;
        return SlashOutcome::Handled;
    }

    match load_resume_plan_for_target(session, target).await {
        Ok(plan) => {
            tracing::info!(
                target: "coco_agent_host::resume",
                session_id = %plan.session_id,
                source_session_id = %plan.source_session_id,
                prior_messages = plan.prior_messages.len(),
                "slash resume: hydrating session",
            );
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "resume",
                args,
                session,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_factory,
                process_runtime,
                runtime_reload_subscriptions,
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode to the resumed session. Runs at a
            // turn boundary, so the env flip is
            // observed by the next prompt assembly.
            if let Some(warning) = coco_agent_host::coordinator_mode_resume::reconcile_on_resume(
                plan.conversation.mode.as_deref(),
                &runtime.runtime_config().features,
            ) {
                emit_slash_text(event_tx, "resume", args, warning).await;
            }
        }
        Err(err) => {
            emit_slash_text(
                event_tx,
                "resume",
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
        }
    }

    SlashOutcome::Handled
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn switch_to_resume_plan_through_app_server(
    plan: &ResumePlan,
    command_name: &str,
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> bool {
    match apply_resume_plan_through_app_server(
        plan,
        session,
        current_session,
        event_tx,
        local_app_server_bridge,
        runtime_factory,
        process_runtime,
        runtime_reload_subscriptions,
    )
    .await
    {
        Ok(()) => true,
        Err(err) => {
            emit_slash_text(
                event_tx,
                command_name,
                args,
                &format!("Failed to resume session: {err}"),
            )
            .await;
            false
        }
    }
}

/// apply the durable-seq skip-ahead
/// when resuming/branching a session so the new epoch never re-issues a seq at
/// or below one already shipped to the Hub (cursor regression when a connector
/// is configured). Reads the resumed transcript's persisted `session_seq`
/// watermark (destination JSONL == source for resume/continue, the copied fork
/// file for `/branch`) and seeds the AppServer retention-ring high-water.
///
/// PARTIAL — the paired `SessionSeqAllocator::initialize_after_watermark`, which
/// actually advances the durable seq *counter*, is NOT applied here. The
/// allocator sits behind `SdkServerState::session_seq_allocator()` (`pub (crate)`
/// in the `coco_cli` lib crate) and the local AppServer bridge exposes no public
/// accessor. `tui_runner` compiles into the separate binary crate, so it cannot
/// reach the allocator without a public accessor being added under
/// `app/agent-host/src/sdk_server/*` (out of scope here). Until then, TUI resume still
/// re-issues seqs from 1 in the new epoch; only the ring watermark is seeded.
pub(super) async fn apply_resume_seq_skip_ahead(
    plan: &ResumePlan,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let transcript_path = plan.destination_path.clone();
    let session_id_str = plan.session_id.to_string();
    let watermark = tokio::task::spawn_blocking(move || {
        coco_session::storage::read_transcript_metadata_at(&transcript_path, &session_id_str)
            .ok()
            .and_then(|meta| meta.session_seq_watermark)
    })
    .await
    .ok()
    .flatten();
    if let Some(watermark) = watermark {
        local_app_server_bridge
            .app_server()
            .initialize_session_ring_watermark(plan.session_id.clone(), watermark);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_resume_plan_through_app_server(
    plan: &ResumePlan,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> anyhow::Result<()> {
    // skip the resumed session's durable-seq epoch above its prior
    // watermark before any new envelope is emitted, covering startup resume,
    // `/resume`, and `/branch` (all three funnel through here).
    apply_resume_seq_skip_ahead(plan, local_app_server_bridge).await;

    let old_session_id = session.current_typed_session_id().await;
    if old_session_id == plan.session_id {
        local_app_server_bridge
            .install_session_runtime(session.clone())
            .await;
        hydrate_runtime_for_resume_plan(session, &plan.session_id, &plan.prior_messages).await;
        local_app_server_bridge.ensure_interactive_surface(plan.session_id.clone())?;
        local_app_server_bridge
            .start_passive_event_pump(plan.session_id.clone(), event_tx.clone())?;
        emit_resume_plan_ui_state_for_runtime(plan, session, event_tx).await;
        return Ok(());
    }

    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;

    let make_runtime_factory = || {
        let runtime_factory = runtime_factory.clone();
        let process_runtime = Arc::clone(process_runtime);
        let cwd = plan.cwd.clone();
        let event_tx = event_tx.clone();
        let session_id = plan.session_id.clone();
        let prior_messages = plan.prior_messages.clone();
        async move {
            build_runtime_for_resume_plan(
                runtime_factory,
                session_id,
                prior_messages,
                process_runtime,
                cwd,
                event_tx,
            )
            .await
        }
    };

    let replacement = local_app_server_bridge
        .replace_session_runtime(
            old_session_id.clone(),
            plan.session_id.clone(),
            make_runtime_factory(),
        )
        .await?;
    let new_session = match replacement {
        Some((session, _surface_id)) => session,
        None => {
            local_app_server_bridge
                .replace_detached_session_runtime(
                    old_session_id,
                    plan.session_id.clone(),
                    make_runtime_factory(),
                )
                .await?
        }
    };

    local_app_server_bridge
        .install_session_runtime(new_session.clone())
        .await;
    {
        let mut current = current_session.write().await;
        *current = new_session.clone();
    }
    runtime_reload_subscriptions
        .lock()
        .await
        .install_for_session(&new_session)
        .await;
    local_app_server_bridge.ensure_interactive_surface(plan.session_id.clone())?;
    local_app_server_bridge.start_passive_event_pump(plan.session_id.clone(), event_tx.clone())?;
    emit_resume_plan_ui_state_for_runtime(plan, &new_session, event_tx).await;
    Ok(())
}

pub(super) async fn build_runtime_for_resume_plan(
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    session_id: coco_types::SessionId,
    prior_messages: Vec<coco_messages::Message>,
    process_runtime: Arc<ProcessRuntime>,
    cwd: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id.clone(), cwd.clone())
        .await?;
    let runtime = &session;
    runtime.install_side_query_event_tx(event_tx.clone()).await;
    hydrate_runtime_for_resume_plan(&session, &session_id, &prior_messages).await;

    let lsp_handle = coco_agent_host::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime,
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    install_session_late_binds(
        session.clone(),
        &cwd,
        None,
        lsp_handle,
        Some(event_tx.clone()),
    )
    .await?;
    coco_agent_host::session_bootstrap::bootstrap_session_mcp(
        &session, &cwd, None, /*await_connect*/ false,
    )
    .await;
    coco_agent_host::leader_inbox_poller::install_leader(session.clone(), None).await;

    runtime.fire_session_start_hooks("resume").await;
    Ok(session)
}

pub(super) async fn build_runtime_for_clear(
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    session_id: coco_types::SessionId,
    permissions: coco_types::LiveToolPermissionState,
    rewind_messages: Option<Vec<Arc<Message>>>,
    process_runtime: Arc<ProcessRuntime>,
    cwd: std::path::PathBuf,
    event_tx: mpsc::Sender<CoreEvent>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id.clone(), cwd.clone())
        .await?;
    let runtime = &session;
    runtime.install_side_query_event_tx(event_tx.clone()).await;
    {
        let mut app_state = runtime.app_state().write().await;
        app_state.permissions = permissions;
    }
    runtime
        .seed_pre_clear_rewind_messages(rewind_messages)
        .await;

    let lsp_handle = coco_agent_host::session_bootstrap::build_lsp_handle_if_enabled(
        process_runtime,
        runtime.runtime_config(),
        &coco_config::global_config::config_home(),
        runtime.project_root(),
    )
    .await;
    install_session_late_binds(
        session.clone(),
        &cwd,
        None,
        lsp_handle,
        Some(event_tx.clone()),
    )
    .await?;
    coco_agent_host::session_bootstrap::bootstrap_session_mcp(
        &session, &cwd, None, /*await_connect*/ false,
    )
    .await;
    coco_agent_host::leader_inbox_poller::install_leader(session.clone(), None).await;

    Ok(session)
}

pub(super) async fn hydrate_runtime_for_resume_plan(
    session: &crate::session_runtime::SessionHandle,
    session_id: &coco_types::SessionId,
    prior_messages: &[coco_messages::Message],
) {
    let runtime = &session;
    {
        let mut history = runtime.history().lock().await;
        history.clear();
        for message in prior_messages.iter().cloned() {
            history.push(message);
        }
    }
    runtime
        .seed_transcript_dedup(prior_messages.iter().filter_map(|m| m.uuid().copied()))
        .await;
    runtime
        .seed_tool_result_replacement_state(prior_messages, session_id, None)
        .await;

    if prior_messages.is_empty() {
        return;
    }
    let cfg = runtime.current_engine_config().await;
    let messages = prior_messages
        .iter()
        .cloned()
        .map(Arc::new)
        .collect::<Vec<_>>();
    let goal = goal_command::restore_goal_from_history(
        &messages,
        runtime.app_state(),
        &runtime.hook_registry(),
        runtime.session_usage_snapshot().await.totals.output_tokens,
        goal_command::GoalGate {
            hooks_restricted: cfg.disable_all_hooks || cfg.allow_managed_hooks_only,
            trust_rejected: false,
        },
    )
    .await;
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

/// `/branch` (alias `/fork`) — fork the current conversation at this point
/// into a NEW session and switch to it live.
/// Copies the current transcript to a fresh uuid via `fork_conversation` (the
/// same primitive `--fork-session` uses), then hydrates the runtime onto the
/// fork through local AppServer `session/resume`. The original session is left
/// untouched on disk.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_branch(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    runtime_factory: &crate::session_runtime::SessionRuntimeFactory,
    process_runtime: &Arc<ProcessRuntime>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) -> SlashOutcome {
    let runtime = session;
    let custom_title = args.trim().to_string();
    let source_id = runtime.current_typed_session_id().await;
    if source_id.as_str().is_empty() {
        emit_slash_text(event_tx, "branch", "", "No active session to branch from.").await;
        return SlashOutcome::Handled;
    }
    let working_dir = runtime.original_cwd().clone();
    let memory_base = runtime.session_manager().memory_base().to_path_buf();
    let plan = tokio::task::spawn_blocking(move || -> anyhow::Result<ResumePlan> {
        let store = coco_session::TranscriptStore::new(std::sync::Arc::new(
            coco_paths::ProjectPaths::new(memory_base, &working_dir),
        ));
        let source_path = store.transcript_path(source_id.as_str());
        if !coco_session::recovery::can_resume_session(&source_path) {
            anyhow::bail!(
                "nothing to branch yet — send a message first so there's a conversation to fork"
            );
        }
        let dest_id = coco_types::SessionId::generate();
        let dest_path = store.transcript_path(dest_id.as_str());
        coco_session::recovery::fork_conversation(&source_path, &dest_path, dest_id.as_str())
            .map_err(|e| {
                anyhow::anyhow!(
                    "fork copy {} → {} failed: {e}",
                    source_path.display(),
                    dest_path.display()
                )
            })?;
        let conversation = coco_session::recovery::load_conversation_for_resume(&dest_path)?;
        let prior_messages = conversation.messages.clone();
        Ok(ResumePlan {
            session_id: dest_id,
            source_session_id: source_id,
            source_path,
            destination_path: dest_path,
            cwd: working_dir,
            prior_messages,
            conversation,
            is_fork: true,
        })
    })
    .await;
    match plan {
        Ok(Ok(plan)) => {
            let new_id = plan.session_id.to_string();
            let source_id = plan.source_session_id.to_string();
            // Derive the fork title: explicit arg, else the first user prompt
            // (truncated), suffixed "(Branch)" — branch.ts. Done
            // BEFORE hydrate moves `plan`.
            let base_title = if custom_title.is_empty() {
                first_user_prompt_title(&plan.prior_messages)
            } else {
                Some(custom_title)
            };
            if !switch_to_resume_plan_through_app_server(
                &plan,
                "branch",
                args,
                session,
                current_session,
                event_tx,
                local_app_server_bridge,
                runtime_factory,
                process_runtime,
                runtime_reload_subscriptions,
            )
            .await
            {
                return SlashOutcome::Handled;
            }
            // Reconcile coordinator mode onto the fork, same as /resume — the
            // fork inherits the source's persisted mode. Runs at a turn
            // boundary so the next prompt assembly observes the flip.
            if let Some(warning) = coco_agent_host::coordinator_mode_resume::reconcile_on_resume(
                plan.conversation.mode.as_deref(),
                &runtime.runtime_config().features,
            ) {
                emit_slash_text(event_tx, "branch", args, warning).await;
            }
            // The fork is now the live session, so session/rename titles it.
            if let Some(base) = base_title {
                let title = format!("{base} (Branch)");
                if let Err(e) = local_app_server_bridge
                    .client()
                    .session_rename(
                        local_app_server_bridge.handler(),
                        coco_types::SessionRenameParams {
                            target: session_target(local_app_server_bridge),
                            name: title,
                        },
                    )
                    .await
                {
                    warn!(error = %e, "failed to set /branch fork title");
                }
            }
            emit_slash_text(
                event_tx,
                "branch",
                args,
                &format!(
                    "Branched into a new session ({new_id}). \
                     To return to the original, /resume {source_id}."
                ),
            )
            .await;
        }
        Ok(Err(e)) => {
            emit_slash_text(event_tx, "branch", "", &format!("Failed to branch: {e}")).await;
        }
        Err(e) => {
            emit_slash_text(event_tx, "branch", "", &format!("Branch task failed: {e}")).await;
        }
    }
    SlashOutcome::Handled
}

/// Derive a short title from the first user message's text (first line,
/// truncated), for naming a `/branch` fork when no explicit title is given.
pub(super) fn first_user_prompt_title(messages: &[coco_messages::Message]) -> Option<String> {
    let text = messages.iter().find_map(|m| {
        matches!(m, coco_messages::Message::User(_))
            .then(|| coco_messages::wrapping::extract_text_from_message(m))
            .filter(|t| !t.trim().is_empty())
    })?;
    let first_line = text.trim().lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let truncated: String = first_line.chars().take(40).collect();
    Some(truncated)
}

pub(super) async fn load_resume_plan_for_target(
    session: &crate::session_runtime::SessionHandle,
    target: &str,
) -> anyhow::Result<ResumePlan> {
    let runtime = session;
    let manager = Arc::clone(runtime.session_manager());
    let target = target.to_string();
    // Project root for THIS runtime. Resume targets whose project root differs
    // from this are cross-project: refuse rather than mid-flight re-point the
    // runtime's transcript store and project-scoped services.
    let runtime_project_root = runtime.project_root().clone();
    tokio::task::spawn_blocking(move || {
        let session = match manager.resume(&target) {
            Ok(session) => session,
            Err(id_err) => {
                resolve_resume_target_by_title(&manager, &target, &runtime_project_root, &id_err)?
            }
        };
        let session_project_root =
            coco_agent_host::paths::resolve_project_root(&session.working_dir);
        if session_project_root != runtime_project_root {
            anyhow::bail!(
                "session {} lives under project {} but this runtime is at {} — \
                 cross-project /resume is not supported. cd to the source \
                 project and try again.",
                session.id,
                session_project_root.display(),
                runtime_project_root.display(),
            );
        }
        let transcript_path = coco_session::TranscriptStore::new(
            coco_agent_host::paths::project_paths(&session.working_dir),
        )
        .transcript_path(&session.id);
        if !coco_session::recovery::can_resume_session(&transcript_path) {
            anyhow::bail!(
                "transcript at {} is empty or unreadable; nothing to resume",
                transcript_path.display()
            );
        }
        let conversation = coco_session::recovery::load_conversation_for_resume(&transcript_path)?;
        let prior_messages = conversation.messages.clone();
        let session_id = coco_types::SessionId::try_new(session.id.clone())
            .map_err(|e| anyhow::anyhow!("invalid session id '{}': {e}", session.id))?;
        Ok(ResumePlan {
            session_id: session_id.clone(),
            source_session_id: session_id,
            source_path: transcript_path.clone(),
            destination_path: transcript_path,
            cwd: session.working_dir,
            prior_messages,
            conversation,
            is_fork: false,
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("resume task failed: {err}"))?
}

/// Case-insensitive exact resolve of `/resume <name>` when the
/// argument doesn't match any session id directly.
/// Returns the unique session on a 1-match (after project filtering),
/// or bails with a diagnostic listing the top-N candidates. The
/// project filter keeps cross-project matches from leaking into the
/// "did you mean X" hint — it would be misleading to suggest a
/// session the runtime can't actually resume.
pub(super) fn resolve_resume_target_by_title(
    manager: &coco_session::SessionManager,
    target: &str,
    runtime_project_root: &std::path::Path,
    id_err: &coco_session::SessionError,
) -> anyhow::Result<coco_session::Session> {
    let mut matches = manager
        .find_by_title(target, true)?
        .into_iter()
        .filter(|s| same_project(&s.working_dir, runtime_project_root))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => anyhow::bail!("no session found for id or title '{target}': {id_err}"),
        1 => Ok(matches.remove(0)),
        n => {
            const MAX_CANDIDATES_SHOWN: usize = 5;
            let lines: Vec<String> = matches
                .iter()
                .take(MAX_CANDIDATES_SHOWN)
                .map(|s| format!("  {}  {}", s.id, s.title.as_deref().unwrap_or("(untitled)")))
                .collect();
            let more = if n > MAX_CANDIDATES_SHOWN {
                format!("\n  …and {} more", n - MAX_CANDIDATES_SHOWN)
            } else {
                String::new()
            };
            anyhow::bail!(
                "ambiguous resume target '{target}' — {n} sessions match. \
                 Re-run with a session id:\n{}{more}",
                lines.join("\n"),
            )
        }
    }
}

pub(super) fn same_project(session_cwd: &std::path::Path, runtime_root: &std::path::Path) -> bool {
    coco_agent_host::paths::resolve_project_root(session_cwd) == runtime_root
}

pub(super) fn session_to_sdk_summary(
    session: coco_session::Session,
) -> Option<coco_types::SdkSessionSummary> {
    let session_id = match coco_types::SessionId::try_new(session.id.clone()) {
        Ok(id) => id,
        Err(err) => {
            warn!(
                session_id = %session.id,
                error = %err,
                "skipping session with invalid id in resume browser"
            );
            return None;
        }
    };
    Some(coco_types::SdkSessionSummary {
        session_id,
        model: session.model,
        cwd: session.working_dir.to_string_lossy().into_owned(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        title: session.title,
        message_count: session.message_count,
        total_tokens: session.total_tokens,
    })
}

pub(super) fn session_plans_dir(
    config_home: &std::path::Path,
    project_dir: Option<&std::path::Path>,
    plans_directory_setting: Option<&str>,
) -> std::path::PathBuf {
    coco_context::resolve_plans_directory(config_home, project_dir, plans_directory_setting)
}

pub(super) fn session_plan_file_path(
    config_home: &std::path::Path,
    project_dir: Option<&std::path::Path>,
    plans_directory_setting: Option<&str>,
    session_id: &coco_types::SessionId,
) -> std::path::PathBuf {
    let plans_dir = session_plans_dir(config_home, project_dir, plans_directory_setting);
    coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None)
}

pub(super) async fn runtime_session_plan_file_path(
    session: &crate::session_runtime::SessionHandle,
) -> std::path::PathBuf {
    let runtime = session;
    let session_id = runtime.current_typed_session_id().await;
    session_plan_file_path(
        runtime.config_home(),
        runtime.runtime_config().paths.project_dir.as_deref(),
        runtime
            .runtime_config()
            .settings
            .merged
            .plans_directory
            .as_deref(),
        &session_id,
    )
}
use std::{collections::HashMap, sync::Arc};

use coco_agent_host::{
    goal_command, resume_resolver::ResumePlan, session_bootstrap::install_session_late_binds,
};
use coco_app_runtime::ProcessRuntime;
use coco_messages::{AssistantContent, LlmMessage, Message};
use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::{Mutex, mpsc};
use tracing::warn;

use super::{
    SharedSessionHandle, SlashOutcome, TuiRuntimeReloadSubscriptions, emit_slash_text,
    session_target, workspace_trust_rejected,
};
