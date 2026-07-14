/// Agent driver — consumes UserCommands, drives QueryEngine, emits CoreEvents.
/// Runs as a background tokio task alongside the TUI event loop.
/// Events flow directly as `CoreEvent` from QueryEngine → TUI (no mapping layer).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_driver(
    mut command_rx: mpsc::Receiver<UserCommand>,
    event_tx: mpsc::Sender<CoreEvent>,
    current_session: SharedSessionHandle,
    mut local_app_server_bridge: coco_agent_host::app_server_host::AppServerLocalBridge,
    pending_approvals: coco_agent_host::tui_permission_bridge::PendingApprovals,
    runtime_reload_subscriptions: Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    cwd: std::path::PathBuf,
    flag_settings: Option<std::path::PathBuf>,
    app_server_shutdown_timeout: Duration,
    event_hub_connector: Option<coco_agent_host::event_hub::ProcessEventHub>,
    event_hub_membership_watcher: Option<tokio::task::JoinHandle<()>>,
    // Cross-process teammate inbox pump (gap 1) completion handshake. When
    // `Some`, each spawned top-level turn fires its `user_message_id` here on
    // completion so the pump can serialize on its own injected turn. `None`
    // for leader / standalone sessions.
    teammate_turn_done_tx: Option<mpsc::Sender<String>>,
) -> coco_agent_host::shutdown::AppServerShutdownOutcome {
    {
        let session = current_session.read().await.clone();
        session.install_side_query_event_tx(event_tx.clone()).await;
    }
    // Per-session one-shot gate: title gen runs at most once per
    // session id, never the process. After `/resume` or `/clear` the
    // session id changes; the new id is not in the set, so the gate
    // re-arms (each session's title state is independent).
    // `Arc<RwLock<HashSet>>` because the SubmitInput body runs in a
    // spawned task — the outer scope must hand a shared handle to the
    // spawn for cross-turn observation.
    let title_gen_attempted: Arc<RwLock<std::collections::HashSet<String>>> =
        Arc::new(RwLock::new(std::collections::HashSet::new()));
    info!("Agent driver started");

    // Active-turn tracker. AppServer-owned turns keep a completion monitor task
    // plus an interrupt client here; the dispatch loop continues to `recv()` so
    // interrupting commands (`Interrupt`, `Compact`, `Rewind`, `Shutdown`)
    // reach their arms without waiting for the engine to finish.
    let active_turn: Arc<Mutex<Option<ActiveTurn>>> = Arc::new(Mutex::new(None));
    let mut pending_editor_requests: HashMap<String, PendingEditorRequest> = HashMap::new();
    let mut explicit_shutdown = false;
    let (turn_done_tx, mut turn_done_rx) = mpsc::channel::<uuid::Uuid>(16);
    let (bash_response_tx, mut bash_response_rx) =
        mpsc::channel::<Vec<Arc<coco_messages::Message>>>(16);

    // Observe SIGINT/SIGTERM for the whole driver lifetime. The TUI runs in raw
    // mode, so Ctrl+C never arrives as SIGINT (it is a key event); this arm is
    // what makes `kill <pid>` mid-turn initiate a graceful drain instead of the
    // default terminate action. Created
    // once so a signal between loop iterations is not missed.
    let mut os_signal = std::pin::pin!(coco_agent_host::shutdown::os_interrupt_signal());

    loop {
        let command = tokio::select! {
            () = &mut os_signal => {
                info!("OS interrupt signal received; draining active turn and shutting down TUI");
                drain_active_turn(
                    &active_turn,
                    ActiveTurnDrain::AbortAfter(TUI_SHUTDOWN_ACTIVE_TURN_DRAIN_TIMEOUT),
                )
                .await;
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::SessionEnded(
                        coco_types::SessionEndedParams {
                            reason: "OS signal".into(),
                        },
                    )))
                    .await;
                explicit_shutdown = true;
                break;
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                command
            }
            Some(turn_id) = turn_done_rx.recv() => {
                if drain_completed_turn(&active_turn, turn_id).await {
                    let session = current_session.read().await.clone();
                    process_idle_command_queue(
                        &session,
                        &current_session,
                        &event_tx,
                        &mut local_app_server_bridge,
                        &active_turn,
                        &mut pending_editor_requests,
                        &title_gen_attempted,
                        &turn_done_tx,
                        &runtime_reload_subscriptions,
                    )
                    .await;
                }
                continue;
            }
            Some(messages) = bash_response_rx.recv() => {
                let session = current_session.read().await.clone();
                spawn_history_turn_through_app_server(
                    messages,
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &active_turn,
                    &turn_done_tx,
                )
                .await;
                continue;
            }
            _ = {
                let session = current_session.read().await.clone();
                async move {
                    coco_agent_host::session_queue::wait_for_command_queue_change(&session).await
                }
            } => {
                let session = current_session.read().await.clone();
                process_idle_command_queue(
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &active_turn,
                    &mut pending_editor_requests,
                    &title_gen_attempted,
                    &turn_done_tx,
                    &runtime_reload_subscriptions,
                )
                .await;
                continue;
            }
        };
        // Re-read each turn so `/clear` regen picks up the new id.
        let session = current_session.read().await.clone();
        let runtime = &session;
        let session_id = runtime.session_id().clone();
        match command {
            UserCommand::SubmitInput {
                user_message_id,
                content,
                images,
                ..
            } => {
                if content.is_empty() {
                    continue;
                }

                // Slash-command interception. When the user typed `/foo args`,
                // resolve through `runtime.command_registry` BEFORE handing
                // raw text to the model.
                let mut effective_content = content;
                let mut slash_metadata = None;
                let mut slash_thinking_level = None;
                let mut slash_model_runtime_source = None;
                if let Some((name, args)) = parse_slash_command(&effective_content) {
                    let outcome = dispatch_slash_command(
                        name,
                        args,
                        &session,
                        &current_session,
                        &event_tx,
                        &mut local_app_server_bridge,
                        &runtime_reload_subscriptions,
                    )
                    .await;
                    let control_context = LocalRuntimeControlContext {
                        current_session: &current_session,
                        runtime_reload_subscriptions: &runtime_reload_subscriptions,
                        turn_done_tx: &turn_done_tx,
                    };
                    match handle_slash_outcome(
                        outcome,
                        &session,
                        &control_context,
                        &event_tx,
                        &active_turn,
                        &mut pending_editor_requests,
                        &mut local_app_server_bridge,
                    )
                    .await
                    {
                        SlashFollowup::Done => continue,
                        // Unknown command falls through to the model
                        // as raw text — falls through to the model.
                        SlashFollowup::NotFound => {}
                        SlashFollowup::RunEngine {
                            content,
                            metadata,
                            thinking_level,
                            model_runtime_source,
                        } => {
                            effective_content = content;
                            slash_metadata = metadata;
                            slash_thinking_level = thinking_level;
                            slash_model_runtime_source = model_runtime_source;
                        }
                    }
                }

                // Defensive drain: TUI input layer gates submit on
                // `running` state, but a slow gate could still let a
                // second SubmitInput through. Cancel + await the prior
                // turn before starting the new one — last-write-wins
                // semantics (a new submit aborts the previous turn).
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;

                let turn_id = uuid::Uuid::new_v4();
                if let Err(error) = local_app_server_bridge.activate_existing_interactive_session(
                    session_id.clone(),
                    Some(event_tx.clone()),
                ) {
                    tracing::warn!(%error, "TUI SubmitInput could not activate local AppServer session");
                    continue;
                }
                let mut monitor_client = local_app_server_bridge.connect_local_client();
                // live-only tail attach with no replay cursor. `Some (0)`
                // falls out of the retention ring on any long-lived/resumed
                // session and returns `SnapshotRequired`, which would drop the
                // submit before `start_turn`. The monitor only needs live events
                // for a turn that hasn't produced output yet.
                let passive_surface = match monitor_client
                    .attach_passive_session(session_id.clone())
                {
                    Ok(surface) => surface,
                    Err(error) => {
                        tracing::warn!(%error, "TUI SubmitInput could not attach AppServer completion monitor");
                        continue;
                    }
                };
                let params = coco_types::TurnStartParams {
                    target: interactive_target(&local_app_server_bridge),
                    prompt: effective_content,
                    history_override: Vec::new(),
                    images: image_data_to_turn_start(&images),
                    slash_metadata,
                    model_selection: model_runtime_source_to_turn_start_selection(
                        slash_model_runtime_source,
                    ),
                    permission_mode: None,
                    thinking_level: slash_thinking_level,
                };
                let started = match start_turn_with_busy_retry(
                    &mut local_app_server_bridge,
                    &session_id,
                    params,
                )
                .await
                {
                    Ok(started) => started,
                    Err(StartTurnBusyError::StillBusy(params)) => {
                        // a prior turn's handler-side state is still draining
                        // after our TUI-side drain + retries. Re-enqueue the prompt
                        // so it runs at the next idle drain instead of dropping the
                        // user's submit.
                        let _ = coco_agent_host::session_queue::enqueue_human_prompt(
                            &session,
                            params.prompt,
                            image_data_to_queued(&images),
                        )
                        .await;
                        tracing::warn!(
                            "TUI SubmitInput turn/start still busy after retries; re-enqueued prompt"
                        );
                        continue;
                    }
                    Err(StartTurnBusyError::Failed(error)) => {
                        tracing::warn!(%error, "TUI SubmitInput AppServer turn/start failed");
                        continue;
                    }
                };
                let session_t = session.clone();
                let title_gen_attempted_t = title_gen_attempted.clone();
                let session_id_t = session_id.clone();
                let turn_done_tx_t = turn_done_tx.clone();
                // Cross-process teammate pump handshake: fire this turn's
                // `user_message_id` on completion (Drop covers panic + cancel).
                let pump_done = teammate_turn_done_tx.as_ref().map(|tx| PumpDoneGuard {
                    id: user_message_id.clone(),
                    tx: tx.clone(),
                });
                let protocol_turn_id = started.turn_id.clone();
                let auto_title_client = local_app_server_bridge.connect_local_client();
                let auto_title_handler = local_app_server_bridge.handler().clone();

                let task = tokio::spawn(async move {
                    let _done = TurnDoneGuard {
                        turn_id,
                        tx: turn_done_tx_t,
                    };
                    let _pump_done = pump_done;
                    let mut auto_title_client = Some(auto_title_client);
                    let mut auto_title_handler = Some(auto_title_handler);
                    while let Some(envelope) =
                        monitor_client.next_passive_event(&passive_surface).await
                    {
                        if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) =
                            envelope.event
                            && ended.turn_id == protocol_turn_id
                        {
                            if let (Some(client), Some(handler)) =
                                (auto_title_client.take(), auto_title_handler.take())
                            {
                                maybe_spawn_auto_title(
                                    &session_t,
                                    &title_gen_attempted_t,
                                    &session_id_t,
                                    client,
                                    handler,
                                )
                                .await;
                            }
                            break;
                        }
                    }
                });
                let interrupt_client = local_app_server_bridge.connect_local_client();
                let handler = local_app_server_bridge.handler().clone();

                *active_turn.lock().await = Some(ActiveTurn {
                    id: turn_id,
                    task,
                    cancel: ActiveTurnCancel {
                        client: interrupt_client,
                        handler,
                        target: interactive_target(&local_app_server_bridge),
                    },
                });
            }

            UserCommand::SubmitBash {
                user_message_id,
                command,
            } => {
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                let event_tx_t = event_tx.clone();
                let session_t = session.clone();
                let bash_response_tx_t = bash_response_tx.clone();
                let cwd = runtime.workspace_cwd().await;
                tokio::spawn(async move {
                    run_prompt_mode_bash(
                        &cwd,
                        user_message_id,
                        command,
                        session_t,
                        event_tx_t,
                        bash_response_tx_t,
                    )
                    .await;
                });
            }

            UserCommand::PersistPromptHistory { display } => {
                // Append to the cross-session composer history off the
                // dispatch thread — the JSONL append takes an advisory file
                // lock.
                let runtime_t = runtime.clone();
                let project = cwd.to_string_lossy().to_string();
                tokio::spawn(async move {
                    if let Err(e) = runtime_t
                        .persist_prompt_history_entry(project, display)
                        .await
                    {
                        warn!(target: "coco_agent_host::history", error = %e,
                            "failed to persist prompt history");
                    }
                });
            }

            UserCommand::OpenMemoryFile { path } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Memory { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPlanEditor => {
                let path = runtime_session_plan_file_path(&session);
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Plan { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPlanPromptEditor {
                request_id,
                initial_content,
                plan_file_path,
            } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::PlanPrompt {
                        request_id,
                        initial_content,
                        path: plan_file_path,
                    },
                    &event_tx,
                )
                .await;
            }

            UserCommand::OpenPromptEditor { initial_content } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Prompt { initial_content },
                    &event_tx,
                )
                .await;
            }

            UserCommand::ExternalEditorTerminalReady { request_id } => {
                let Some(request) = pending_editor_requests.remove(&request_id) else {
                    warn!(%request_id, "terminal ready for unknown external editor request");
                    continue;
                };
                match request {
                    PendingEditorRequest::Memory { path } => {
                        run_open_memory_file(path, event_tx.clone()).await;
                    }
                    PendingEditorRequest::Plan { path } => {
                        run_open_plan_file(path, event_tx.clone()).await;
                    }
                    PendingEditorRequest::PlanPrompt {
                        request_id,
                        initial_content,
                        path,
                    } => {
                        run_plan_prompt_editor(request_id, initial_content, path, event_tx.clone())
                            .await;
                    }
                    PendingEditorRequest::Prompt { initial_content } => {
                        run_prompt_editor(initial_content, event_tx.clone()).await;
                    }
                    PendingEditorRequest::Agent { path } => {
                        run_open_agent_file(session.clone(), path, event_tx.clone()).await;
                    }
                }
            }

            UserCommand::ExternalEditorTerminalPrepareFailed { request_id, error } => {
                let Some(request) = pending_editor_requests.remove(&request_id) else {
                    warn!(%request_id, "terminal prepare failed for unknown editor request");
                    continue;
                };
                emit_editor_prepare_failed(request, error, event_tx.clone()).await;
            }

            UserCommand::SetModelRole {
                role,
                provider,
                model_id,
                effort,
            } => {
                apply_role_through_app_server(
                    &session,
                    role,
                    provider,
                    model_id,
                    effort,
                    &event_tx,
                    &local_app_server_bridge,
                )
                .await;
            }

            UserCommand::ProviderLogin { provider } => {
                // `/login` picker Enter — run the OAuth flow for the chosen
                // instance in the background (the flow blocks on a loopback
                // callback for up to 5 min). On success `dispatch_provider_login`
                // emits the transcript message + `ProviderStatusesRefreshed`, so
                // the `/model` picker's `NotLoggedIn` gate clears live.
                let session_t = session.clone();
                let event_tx_t = event_tx.clone();
                tokio::spawn(async move {
                    dispatch_provider_login(&provider, &session_t, &event_tx_t).await;
                });
            }

            UserCommand::SetThinkingLevel { level } => {
                set_thinking_level_through_app_server(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    level,
                )
                .await;
            }

            UserCommand::ToggleFastMode => {
                toggle_fast_mode_through_app_server(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                )
                .await;
            }

            UserCommand::ExecuteSkill { name, args } => {
                // Command-palette dispatch.
                // Same registry lookup as the typed path, but with no
                // user-supplied chat message — for `Prompt` outcomes
                // [`spawn_slash_run_engine_turn`] mints a fresh
                // user-message UUID so file-history / rewind keys
                // line up.
                let args_str = args.unwrap_or_default();
                let outcome = dispatch_slash_command(
                    &name,
                    &args_str,
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &runtime_reload_subscriptions,
                )
                .await;
                let control_context = LocalRuntimeControlContext {
                    current_session: &current_session,
                    runtime_reload_subscriptions: &runtime_reload_subscriptions,
                    turn_done_tx: &turn_done_tx,
                };
                match handle_slash_outcome(
                    outcome,
                    &session,
                    &control_context,
                    &event_tx,
                    &active_turn,
                    &mut pending_editor_requests,
                    &mut local_app_server_bridge,
                )
                .await
                {
                    SlashFollowup::Done => {}
                    SlashFollowup::NotFound => {
                        warn!(%name, "ExecuteSkill: command not registered");
                    }
                    SlashFollowup::RunEngine {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    } => {
                        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                        spawn_slash_run_engine_turn(
                            SlashEnginePrompt {
                                content,
                                metadata,
                                thinking_level,
                                model_runtime_source,
                            },
                            &session,
                            &event_tx,
                            &mut local_app_server_bridge,
                            &active_turn,
                            &title_gen_attempted,
                            &turn_done_tx,
                            &session_id,
                        )
                        .await;
                    }
                }
            }

            UserCommand::ExecuteSlashCommand { name, args } => {
                let refresh_plugin_dialog = name.as_str() == "plugin";
                let outcome = dispatch_slash_command(
                    name.as_str(),
                    &args,
                    &session,
                    &current_session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &runtime_reload_subscriptions,
                )
                .await;
                let control_context = LocalRuntimeControlContext {
                    current_session: &current_session,
                    runtime_reload_subscriptions: &runtime_reload_subscriptions,
                    turn_done_tx: &turn_done_tx,
                };
                match handle_slash_outcome(
                    outcome,
                    &session,
                    &control_context,
                    &event_tx,
                    &active_turn,
                    &mut pending_editor_requests,
                    &mut local_app_server_bridge,
                )
                .await
                {
                    SlashFollowup::Done => {}
                    SlashFollowup::NotFound => {
                        emit_slash_status(
                            &event_tx,
                            name.as_str(),
                            &args,
                            SlashCommandStatusKind::NoHandler,
                        )
                        .await;
                    }
                    SlashFollowup::RunEngine {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    } => {
                        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                        spawn_slash_run_engine_turn(
                            SlashEnginePrompt {
                                content,
                                metadata,
                                thinking_level,
                                model_runtime_source,
                            },
                            &session,
                            &event_tx,
                            &mut local_app_server_bridge,
                            &active_turn,
                            &title_gen_attempted,
                            &turn_done_tx,
                            &session_id,
                        )
                        .await;
                    }
                }
                if refresh_plugin_dialog {
                    refresh_plugin_dialog_payload(&session, &event_tx).await;
                }
            }

            UserCommand::Rewind { message_id, mode } => {
                // Drain first — rewind reads file_history snapshots
                // and rewrites runtime.history(); an in-flight turn that
                // mutates either would race.
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                match mode {
                    coco_tui::command::RewindMode::Explicit {
                        restore_type,
                        rewound_turn,
                    } => {
                        handle_rewind(
                            &restore_type,
                            &message_id,
                            rewound_turn,
                            &event_tx,
                            &session,
                            &mut local_app_server_bridge,
                        )
                        .await;
                    }
                    coco_tui::command::RewindMode::AutoRestore => {
                        handle_auto_truncate(&message_id, &event_tx, &session).await;
                    }
                }
            }

            UserCommand::RequestPermissionExplanation {
                request_id,
                tool_name,
                tool_input,
            } => {
                // Lazily fetch the risk explanation off the hot path (Ctrl+E
                // panel) and reply with PermissionExplanationReady.
                // `explain_permission_risk` gates on the setting + bounds the
                // call, so a disabled/slow explainer resolves to None and the
                // panel shows "unavailable".
                let runtime = runtime.clone();
                let tx = event_tx.clone();
                tokio::spawn(async move {
                    let params = coco_permissions::ExplainerParams {
                        tool_name: &tool_name,
                        tool_input: &tool_input,
                        tool_description: None,
                        messages: None,
                    };
                    let explanation = runtime.explain_permission_risk(params).await;
                    let _ = tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::PermissionExplanationReady {
                            request_id,
                            explanation,
                        }))
                        .await;
                });
            }

            UserCommand::RequestDiffStats { message_id } => {
                // Async restore-preview diff for the selected checkpoint.
                // `stats == None` carries `fileHistoryCanRestore == false`;
                // the TUI suppresses code-restore choices for that row.
                let stats = match coco_agent_host::session_controls::rewind_diff_stats(
                    Some(runtime.clone()),
                    &message_id,
                )
                .await
                {
                    Ok(Some(stats)) => Some(diff_stats_to_payload(stats)),
                    Ok(None) => None,
                    Err(
                        coco_agent_host::session_controls::SessionControlError::FileDiffNotEnabled,
                    ) => continue,
                    Err(_) => Some(coco_types::RewindDiffStatsPayload::default()),
                };
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::RewindRestorePreviewReady {
                        message_id,
                        stats,
                    }))
                    .await;
            }
            UserCommand::RequestDiffStatsBatch { message_ids } => {
                // For each non-synthetic picker row, resolve
                // `fileHistoryCanRestore` and (if restorable) compute
                // the per-row `+X -Y` diff against the next row's
                // snapshot — or the working tree for the last row.
                // Uses the snapshot pair instead of walking
                // `msg.toolUseResult.structuredPatch` because
                // coco_messages has no typed tool-output side channel.
                let mut rows = Vec::with_capacity(message_ids.len());
                for (idx, message_id) in message_ids.iter().enumerate() {
                    let next = message_ids.get(idx + 1).map(String::as_str);
                    let metadata = match coco_agent_host::session_controls::rewind_diff_stats_between(
                        Some(runtime.clone()),
                        message_id,
                        next,
                    )
                    .await
                    {
                        Ok(Some(stats)) => Some(diff_stats_to_payload(stats)),
                        Ok(None) => None,
                        Err(coco_agent_host::session_controls::SessionControlError::FileDiffNotEnabled) => {
                            rows.clear();
                            break;
                        }
                        Err(_) => Some(coco_types::RewindDiffStatsPayload::default()),
                    };
                    rows.push(coco_types::RewindRowMetadata {
                        message_id: message_id.clone(),
                        metadata,
                    });
                }
                if rows.is_empty() && !message_ids.is_empty() {
                    continue;
                }
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::RewindRowMetadataReady {
                        rows,
                    }))
                    .await;
            }

            UserCommand::Interrupt(_reason) => {
                // Mid-turn cancel now flows through the same AppServer
                // `turn/interrupt` request the SDK uses. The task slot stays
                // Some until the turn naturally emits its terminal event; the
                // next SubmitInput or driver shutdown drains it if needed.
                if let Some(state) = active_turn.lock().await.as_ref() {
                    let ActiveTurnCancel {
                        client,
                        handler,
                        target,
                    } = &state.cancel;
                    match client.turn_interrupt(handler, target.clone()).await {
                        Ok(()) => info!("Interrupt: cancelled AppServer active turn"),
                        Err(error) => {
                            tracing::warn!(%error, "Interrupt: AppServer turn/interrupt failed")
                        }
                    }
                }
            }

            UserCommand::InterruptAgentCurrentWork { agent_id } => {
                if let Err(error) = local_app_server_bridge
                    .activate_existing_interactive_session(session.session_id().clone(), None)
                {
                    tracing::warn!(%agent_id, %error, "Interrupt: could not activate local AppServer session");
                    continue;
                }
                match local_app_server_bridge
                    .client()
                    .agent_interrupt_current_work(
                        local_app_server_bridge.handler(),
                        coco_types::AgentInterruptCurrentWorkParams {
                            target: interactive_target(&local_app_server_bridge),
                            agent_id: agent_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => info!(%agent_id, "Interrupt: cancelled teammate current turn"),
                    Err(error) => {
                        tracing::warn!(%agent_id, %error, "Interrupt: teammate current turn failed")
                    }
                }
            }

            UserCommand::OpenAgentEditor { path } => {
                prepare_external_editor_request(
                    &mut pending_editor_requests,
                    PendingEditorRequest::Agent { path },
                    &event_tx,
                )
                .await;
            }

            UserCommand::CreateAgent {
                name,
                description,
                source,
            } => {
                // The TUI wizard pre-flights the writable-source +
                // file-exists checks before dispatching, so by the
                // time we reach here only rare I/O races land in the
                // error arm. Toast the failure and move on — the
                // wizard is already closed.
                match coco_agent_host::session_agents::prepare_agent_create(
                    &session,
                    &name,
                    &description,
                    source,
                )
                .await
                {
                    Ok(path) => {
                        prepare_external_editor_request(
                            &mut pending_editor_requests,
                            PendingEditorRequest::Agent { path },
                            &event_tx,
                        )
                        .await;
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "coco::agents",
                            error = %err.to_user_string(),
                            %name,
                            ?source,
                            "CreateAgent: prepare failed"
                        );
                        let _ = event_tx
                            .send(CoreEvent::Tui(TuiOnlyEvent::PromptEditorFailed {
                                error: err.to_user_string(),
                            }))
                            .await;
                    }
                }
            }

            UserCommand::DeleteAgentFile { path } => {
                let session_t = session.clone();
                let event_tx_t = event_tx.clone();
                let path_display = path.display().to_string();
                tokio::spawn(async move {
                    if let Err(err) =
                        coco_agent_host::session_agents::delete_agent_file(&session_t, path).await
                    {
                        tracing::warn!(
                            target: "coco::agents",
                            %path_display,
                            error = %err,
                            "DeleteAgentFile: remove failed"
                        );
                        return;
                    }
                    // After delete, rebuild the payload and re-push so
                    // the dialog refreshes without the deleted row.
                    refresh_agents_dialog(&session_t, &event_tx_t).await;
                });
            }

            UserCommand::CancelSubagent { task_id } => {
                // Fire the cancel token on the running task. The
                // existing task-driver pipeline emits
                // `CoreEvent::Protocol (TaskCompleted { status: Stopped })`
                // when the cancel takes effect, which the TUI handler
                // folds into `SessionState.subagents` so the Running
                // tab refreshes on the next frame. No additional event
                // wiring needed here.
                if let Err(error) = local_app_server_bridge
                    .activate_existing_interactive_session(session.session_id().clone(), None)
                {
                    tracing::warn!(%task_id, %error, "CancelSubagent: could not activate local AppServer session");
                    continue;
                }
                match local_app_server_bridge
                    .client()
                    .stop_task(
                        local_app_server_bridge.handler(),
                        coco_types::StopTaskParams {
                            target: interactive_target(&local_app_server_bridge),
                            task_id: task_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(()) => info!(%task_id, "CancelSubagent: cancel token fired"),
                    Err(error) => tracing::warn!(
                        %task_id,
                        %error,
                        "CancelSubagent: AppServer stopTask failed"
                    ),
                }
            }

            UserCommand::QueueCommand { prompt, images } => {
                // User typed Enter while the agent was streaming.
                // Push onto the session-scoped command queue so the
                // running engine sees it at its next drain point
                // (mid-turn `Now` drain or end-of-turn full drain).
                // When a turn is active, the prompt is enqueued instead
                // of starting a fresh turn.
                let Some(queued) = coco_agent_host::session_queue::enqueue_human_prompt(
                    &session,
                    prompt,
                    image_data_to_queued(&images),
                )
                .await
                else {
                    continue;
                };
                // Round-trip notify: the TUI display
                // (`SessionState::queued_commands`) is a projection of
                // engine state and waits for this event to update —
                // see `update.rs::QueueInput` (no optimistic push).
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::CommandQueued {
                        id: queued.id.to_string(),
                        preview: queued.preview,
                        editable: queued.editable,
                    }))
                    .await;
            }

            UserCommand::EditQueuedCommand { id } => {
                let queued = match coco_agent_host::session_queue::remove_queued_command_for_edit(
                    &session, &id,
                )
                .await
                {
                    Ok(queued) => queued,
                    Err(error) => {
                        let _ = event_tx
                            .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditUnavailable {
                                id,
                                reason: error.to_string(),
                            }))
                            .await;
                        continue;
                    }
                };
                let id = queued.id.clone();
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                        id: id.clone(),
                    }))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditReady {
                        id,
                        prompt: queued.prompt,
                        images: queued.images,
                    }))
                    .await;
            }

            UserCommand::EditQueuedCommands {
                current_input,
                current_cursor,
            } => {
                let queued =
                    match coco_agent_host::session_queue::dequeue_editable_commands_for_edit(
                        &session,
                        &current_input,
                        current_cursor,
                    )
                    .await
                    {
                        Ok(queued) => queued,
                        Err(error) => {
                            let _ = event_tx
                                .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandEditUnavailable {
                                    id: String::new(),
                                    reason: error.to_string(),
                                }))
                                .await;
                            continue;
                        }
                    };

                for id in &queued.ids {
                    let _ = event_tx
                        .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                            id: id.clone(),
                        }))
                        .await;
                }
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
                        queued: queued.remaining_queued as i32,
                    }))
                    .await;
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::QueuedCommandsEditReady {
                        ids: queued.ids,
                        prompt: queued.prompt,
                        cursor: queued.cursor,
                        images: queued.images,
                    }))
                    .await;
            }

            UserCommand::Compact {
                custom_instructions,
            } => {
                // Manual `/compact [instructions]` from the TUI.
                // `custom_instructions` comes from trimming the args.
                info!(
                    session_id = %session_id,
                    has_instructions = custom_instructions.is_some(),
                    "TUI: manual /compact"
                );
                run_manual_compact(
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    custom_instructions,
                    &active_turn,
                    &turn_done_tx,
                )
                .await;
            }

            UserCommand::SetPermissionMode { mode } => {
                let permission_status =
                    match coco_agent_host::session_controls::permission_mode_status(Some(
                        session.clone(),
                    ))
                    .await
                    {
                        Ok(status) => status,
                        Err(error) => {
                            warn!(
                                error = %error,
                                "TUI SetPermissionMode could not read permission mode status"
                            );
                            continue;
                        }
                    };
                if mode == coco_types::PermissionMode::BypassPermissions
                    && !permission_status.bypass_available
                {
                    warn!(
                        session_id = %permission_status.session_id,
                        requested = ?mode,
                        "TUI SetPermissionMode denied: bypass capability gate is off"
                    );
                    continue;
                }
                if let Err(error) = local_app_server_bridge.activate_existing_interactive_session(
                    session.session_id().clone(),
                    Some(event_tx.clone()),
                ) {
                    warn!(
                        session_id = %permission_status.session_id,
                        error = %error,
                        "TUI SetPermissionMode could not activate local AppServer session"
                    );
                    continue;
                }
                if let Err(error) = local_app_server_bridge
                    .client()
                    .set_permission_mode(
                        local_app_server_bridge.handler(),
                        coco_types::SetPermissionModeParams {
                            target: interactive_target(&local_app_server_bridge),
                            mode,
                        },
                    )
                    .await
                {
                    warn!(
                        session_id = %permission_status.session_id,
                        requested = ?mode,
                        error = %error,
                        "TUI SetPermissionMode via AppServerLocalBridge failed"
                    );
                    continue;
                }
                info!(
                    session_id = %permission_status.session_id,
                    from = ?permission_status.current,
                    to = ?mode,
                    "TUI SetPermissionMode propagated to engine_config + app_state",
                );
                // If THIS session is a teammate (cross-process pane), mirror
                // the new mode into team.json so the leader's roster reflects
                // it — covers both an inbox-applied `ModeSetRequest` and a
                // self-initiated Shift+Tab cycle. Leader sessions are not
                // teammates, so this no-ops.
                if coco_coordinator::identity::is_teammate()
                    && let Some(team) = coco_coordinator::identity::get_team_name()
                    && let Some(agent) = coco_coordinator::identity::get_agent_name()
                    && let Err(e) = coco_coordinator::team_file::set_member_mode_in_team_file(
                        &team, &agent, mode,
                    )
                {
                    warn!(error = %e, "teammate mode write-back to team.json failed");
                }
            }

            UserCommand::SetTeammateMode { name, mode } => {
                // Leader applies a teammate's mode from the roster picker
                // (gap 8): persist to team.json + notify the live teammate.
                if let Some(handle) = runtime.current_agent_handle().await {
                    match handle.set_teammate_mode(&name, mode).await {
                        Ok(msg) => info!(teammate = %name, ?mode, "{msg}"),
                        Err(e) => {
                            warn!(teammate = %name, error = %e, "set teammate mode failed")
                        }
                    }
                }
            }

            UserCommand::SetTeammateModes { updates } => {
                // Leader applies the roster "cycle all" batch: one atomic
                // team.json write + a ModeSetRequest per teammate.
                if let Some(handle) = runtime.current_agent_handle().await {
                    match handle.set_teammate_modes(updates).await {
                        Ok(msg) => info!("{msg}"),
                        Err(e) => warn!(error = %e, "set teammate modes (cycle all) failed"),
                    }
                }
            }

            UserCommand::PlanApprovalResponse {
                request_id,
                teammate_agent,
                approved,
                feedback,
            } => {
                // Leader responding to a teammate's plan-approval
                // request. Write a `PlanApprovalResponse` envelope into
                // the teammate's inbox; their `poll_teammate_approval`
                // picks it up on the next turn boundary.
                let team_name = match env::var(EnvKey::CocoTeamName) {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        info!(%request_id, "PlanApprovalResponse: no COCO_TEAM_NAME; dropping");
                        continue;
                    }
                };
                let agent_name =
                    env::var(EnvKey::CocoAgentName).unwrap_or_else(|_| "team-lead".to_string());
                let mailbox: coco_tool_runtime::MailboxHandleRef =
                    Arc::new(coco_coordinator::mailbox::SwarmMailboxHandle);

                let response = coco_tool_runtime::PlanApprovalMessage::PlanApprovalResponse(
                    coco_tool_runtime::PlanApprovalResponse {
                        request_id: request_id.clone(),
                        approved,
                        feedback: feedback.clone(),
                        permission_mode: None,
                    },
                );
                let envelope = coco_tool_runtime::MailboxEnvelope {
                    text: serde_json::to_string(&response).unwrap_or_default(),
                    from: agent_name.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    summary: Some("plan approval response".to_string()),
                };
                if let Err(e) = mailbox
                    .write_to_mailbox(&teammate_agent, &team_name, envelope)
                    .await
                {
                    info!(%request_id, error = %e, "failed to write PlanApprovalResponse");
                } else {
                    // Clear the leader-side awaiting flag so the
                    // reminder can stop nagging about this request.
                    runtime
                        .clear_awaiting_plan_approval_if_matches(&request_id)
                        .await;
                }
            }

            UserCommand::ApprovalResponse {
                request_id,
                approved,
                always_allow,
                feedback,
                updated_input,
                resolution_detail,
                mut permission_updates,
                content_blocks,
            } => {
                let pending_entry = coco_agent_host::tui_permission_bridge::take_pending(
                    &pending_approvals,
                    &request_id,
                )
                .await;

                let always_allow_options_allowed =
                    coco_agent_host::tui_permission_bridge::session_allows_always_allow_options(
                        runtime,
                    );
                if pending_entry.is_some()
                    && !always_allow_options_allowed
                    && !permission_updates.is_empty()
                {
                    warn!(
                        %request_id,
                        "dropping permission updates because managed policy disables always-allow"
                    );
                    permission_updates.clear();
                }

                // Apply any rule additions the user authorized
                // ("Always Allow" or future destination-picker
                // selections) BEFORE resolving the bridge. Order
                // Order: apply before resolving bridge so subsequent same-tool
                // calls within the turn pick up the rule.
                let mut applied_permission_updates = Vec::new();
                if pending_entry.is_some() && approved && !permission_updates.is_empty() {
                    // Unified apply through local AppServer before resolving
                    // the bridge, so subsequent same-tool calls in this turn
                    // pick up the rule through the runtime-control path.
                    for update in &permission_updates {
                        if apply_and_persist_permission_update(
                            update,
                            &event_tx,
                            &local_app_server_bridge,
                        )
                        .await
                        {
                            applied_permission_updates.push(update.clone());
                        }
                    }

                    // `command_permissions` carries command-scoped allowed
                    // tools. Do not encode deny/ask/reset events as
                    // `allowedTools`. One `command_permissions` attachment
                    // is emitted per slash-command invocation — event-stream
                    // semantics, not a snapshot.
                    let mut allowed_tools = Vec::new();
                    for update in &applied_permission_updates {
                        if let coco_types::PermissionUpdate::AddRules { rules, destination } =
                            update
                        {
                            for rule in rules {
                                if matches!(
                                    destination,
                                    coco_types::PermissionUpdateDestination::Command
                                ) && rule.behavior == coco_types::PermissionBehavior::Allow
                                {
                                    allowed_tools.push(rule.value.tool_pattern.clone());
                                }
                            }
                        }
                    }
                    if !allowed_tools.is_empty() {
                        let emitter = runtime.attachment_emitter();
                        emitter.emit(
                            coco_messages::AttachmentMessage::silent_command_permissions(
                                coco_messages::CommandPermissionsPayload {
                                    allowed_tools,
                                    model: None,
                                },
                            ),
                        );
                    }
                }

                // Always-allow with empty `permission_updates` is the
                // legacy path (pre-Phase A). Treat as one-shot approve
                // — the rule plumbing the prompt produced was lost
                // somewhere between TUI and runner. Log and move
                // on rather than failing.
                if always_allow && permission_updates.is_empty() {
                    debug!(
                        %request_id,
                        "always_allow set without permission_updates; treating as one-shot approve"
                    );
                }

                // Route the user's Approve / Deny back to the pending
                // oneshot the `TuiPermissionBridge` is awaiting.
                // `applied_updates` are forwarded so audit/logging
                // downstream sees the user's intent. Stale request_ids
                // (already resolved or timed-out) are logged and
                // dropped when stale (already resolved or timed-out).
                if let Some(entry) = pending_entry {
                    let resolved = coco_agent_host::tui_permission_bridge::send_resolution(
                        entry,
                        approved,
                        feedback,
                        applied_permission_updates,
                        updated_input,
                        resolution_detail,
                        content_blocks,
                    );
                    if !resolved {
                        info!(
                            %request_id,
                            approved,
                            "ApprovalResponse receiver dropped after request was taken"
                        );
                    }
                } else {
                    info!(
                        %request_id,
                        approved,
                        "ApprovalResponse for unknown request_id (already resolved or stale)"
                    );
                }
            }

            UserCommand::ApplyPermissionUpdate { update } => {
                // `/permissions` editor add / delete. Apply to the live
                // engine config + persist to the chosen settings file
                // (User / Project / Local), then re-emit the editor payload
                // so the open overlay refreshes from disk.
                let _ = apply_and_persist_permission_update(
                    &update,
                    &event_tx,
                    &local_app_server_bridge,
                )
                .await;
                refresh_permissions_editor(&session, &event_tx).await;
            }

            UserCommand::Shutdown { reason } => {
                info!(%reason, "Shutdown requested by TUI");
                // User-confirmed exit must return control to the
                // terminal promptly. Give an in-flight turn a short
                // cooperative-cancel window, but do not wait on the
                // long memory drains here; those can take up to 75s
                // and make the double-press exit look ignored.
                drain_active_turn(
                    &active_turn,
                    ActiveTurnDrain::AbortAfter(TUI_SHUTDOWN_ACTIVE_TURN_DRAIN_TIMEOUT),
                )
                .await;
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::SessionEnded(
                        coco_types::SessionEndedParams {
                            reason: "User shutdown".into(),
                        },
                    )))
                    .await;
                explicit_shutdown = true;
                break;
            }

            UserCommand::FireIdleNotification { message } => {
                runtime
                    .fire_notification_hooks("idle_prompt", &message, /*title*/ None)
                    .await;
            }

            UserCommand::BackgroundAllTasks => {
                // Ctrl+B single press: flip every foreground BgAgent /
                // Shell row to backgrounded server-side. The TUI mirror
                // in `session.subagents` is updated optimistically inside
                // `TuiCommand::BackgroundAllTasks` (update.rs) — foreground→background
                // is a UI-state transition,
                // not a task lifecycle event, so no `task/*` wire event
                // fires now. The eventual real `task/completed` (with
                // `output_file` populated) flows when the bg task
                // actually terminates. Idempotent — a second press with
                // no foreground tasks transitions nothing.
                match background_all_tasks_through_app_server(
                    &session,
                    &mut local_app_server_bridge,
                )
                .await
                {
                    Ok(result) => {
                        let count = result.len();
                        info!(count, "BackgroundAllTasks: backgrounded foreground tools");
                    }
                    Err(error) => {
                        warn!(%error, "BackgroundAllTasks via AppServerLocalBridge failed");
                    }
                }
            }

            UserCommand::PushSystemMessage { kind } => {
                // TUI-originated transcript content (slash output,
                // file-open notices, plan-rejected body, …) round-trips
                // through engine `MessageHistory` so every observer
                // (TUI transcript view, SDK consumers, JSONL transcript)
                // sees it via the same `MessageAppended` event stream as
                // engine-pushed content. See
                // `engine-tui-unified-transcript-plan.md` §3 Commit 2.
                let push_message = tui_system_push_message(kind);
                runtime
                    .append_messages_to_history_and_emit(vec![push_message], Some(event_tx.clone()))
                    .await;
            }

            UserCommand::RetryPermissionDenied { tool_name, message } => {
                drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
                let push_message =
                    tui_system_push_message(coco_tui::SystemPushKind::PermissionRetry {
                        tool_name,
                        message,
                    });
                let messages = runtime
                    .append_messages_to_history_and_emit(vec![push_message], Some(event_tx.clone()))
                    .await;
                spawn_history_turn_through_app_server(
                    messages,
                    &session,
                    &event_tx,
                    &mut local_app_server_bridge,
                    &active_turn,
                    &turn_done_tx,
                )
                .await;
            }

            UserCommand::PushSlashResult { entry } => {
                // TUI owns localized text and interaction policy; host owns
                // the engine message envelopes and transcript authority.
                match entry {
                    coco_tui::SlashTranscriptEntry::Result {
                        name,
                        args,
                        text,
                        is_error,
                    } => {
                        coco_agent_host::session_messages::append_slash_result_to_history_and_emit(
                            runtime,
                            event_tx.clone(),
                            &name,
                            &args,
                            &text,
                            is_error,
                        )
                        .await;
                    }
                    coco_tui::SlashTranscriptEntry::ContextUsage { args, result } => {
                        coco_agent_host::session_messages::append_context_usage_to_history_and_emit(
                            runtime,
                            event_tx.clone(),
                            &args,
                            *result,
                        )
                        .await;
                    }
                }
            }

            UserCommand::WriteSkillOverrides { patch } => {
                let runtime_publisher = runtime.runtime_publisher();
                handle_write_skill_overrides(
                    &session,
                    &event_tx,
                    patch,
                    runtime_publisher.as_ref(),
                    &cwd,
                    flag_settings.as_deref(),
                )
                .await;
            }

            // Other commands: log and skip for now
            other => {
                info!(?other, "Unhandled UserCommand in agent driver");
            }
        }
    }

    // Driver loop exited (sender dropped or Shutdown). Drain any
    // turn that's still running so we don't leak a JoinHandle, and
    // wait briefly on any pending auto-memory extraction so partial
    // writes don't get cut off.
    if explicit_shutdown {
        debug!("skipping memory extraction drain after explicit TUI shutdown");
    } else {
        drain_active_turn(&active_turn, ActiveTurnDrain::Wait).await;
        let session = current_session.read().await.clone();
        drain_pending_memory_extraction(&session).await;
    }
    let shutdown_coordinator = coco_agent_host::shutdown::ShutdownCoordinator::new(
        "local TUI",
        app_server_shutdown_timeout,
    );
    let app_server_shutdown = shutdown_coordinator
        .drain_app_server(local_app_server_bridge.shutdown_registered_sessions())
        .await;
    // Re-append metadata one more time at process-exit so the tail window
    // of the final transcript JSONL definitely carries the user's
    // title/tag/agent-name. Best-effort — IO errors here are logged but
    // don't propagate out of the driver.
    {
        let session = current_session.read().await.clone();
        coco_agent_host::shutdown::flush_interactive_session_exit_checkpoint(&session).await;
    }
    let shutdown = shutdown_coordinator
        .finish_after_app_server(
            app_server_shutdown,
            event_hub_connector,
            event_hub_membership_watcher,
        )
        .await;
    info!("Agent driver stopped");
    shutdown
}

/// Wait for scheduled turn-end extraction/session-memory work before
/// shutdown. Silently no-ops when auto-memory is inactive.
async fn drain_pending_memory_extraction(session: &crate::session_runtime::SessionHandle) {
    let Some(memory_runtime) = session.memory_runtime() else {
        return;
    };
    if !memory_runtime
        .drain(coco_memory::service::extract::DEFAULT_DRAIN_TIMEOUT)
        .await
    {
        warn!("auto-memory extraction did not drain within timeout — continuing shutdown");
    }
}

/// Convert a TUI-originated `SystemPushKind` into a transcript `Message`. Lives
/// in the TUI surface (coco-cli) so `coco-agent-host` does not depend on
/// `coco-tui` for system-message construction (Phase G).
pub(crate) fn tui_system_push_message(kind: coco_tui::SystemPushKind) -> coco_messages::Message {
    let system = match kind {
        coco_tui::SystemPushKind::Informational {
            level,
            title,
            message,
        } => {
            coco_messages::SystemMessage::Informational(coco_messages::SystemInformationalMessage {
                uuid: uuid::Uuid::new_v4(),
                level,
                title,
                message,
            })
        }
        coco_tui::SystemPushKind::LocalCommand { command, output } => {
            coco_messages::SystemMessage::LocalCommand(coco_messages::SystemLocalCommandMessage {
                uuid: uuid::Uuid::new_v4(),
                command,
                output,
            })
        }
        coco_tui::SystemPushKind::PermissionRetry { tool_name, message } => {
            coco_messages::SystemMessage::PermissionRetry(
                coco_messages::SystemPermissionRetryMessage {
                    uuid: uuid::Uuid::new_v4(),
                    tool_name,
                    message,
                },
            )
        }
    };
    coco_messages::Message::System(system)
}

use super::*;
