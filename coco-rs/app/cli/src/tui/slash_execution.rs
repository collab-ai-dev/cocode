use super::*;
pub(super) async fn background_all_tasks_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) -> Result<Vec<String>, coco_app_server_client::ClientError> {
    local_app_server_bridge
        .activate_existing_interactive_session(session.session_id().clone(), None)?;
    local_app_server_bridge
        .client()
        .background_all_tasks(
            local_app_server_bridge.handler(),
            interactive_session(local_app_server_bridge),
        )
        .await
        .map(|result| result.task_ids)
}

pub(super) async fn spawn_command_queue_turn(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    let Some(batch) =
        coco_agent_host::session_queue::dequeue_next_prompt_batch(session, Some(event_tx.clone()))
            .await
    else {
        return;
    };

    for id in batch.ids {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                id,
            }))
            .await;
    }
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
            queued: batch.remaining_queued as i32,
        }))
        .await;

    spawn_history_turn_through_app_server(
        batch.messages,
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
    )
    .await;
}

/// bounded retries when local `turn/start` transiently reports
/// `TurnAlreadyRunning` because a prior turn's handler-side state is still
/// draining even though our TUI-side `drain_active_turn` already returned.
pub(super) const START_TURN_BUSY_RETRIES: u32 = 5;
pub(super) const START_TURN_BUSY_BACKOFF: std::time::Duration =
    std::time::Duration::from_millis(20);

/// Terminal outcome of [`start_turn_with_busy_retry`] when it does not succeed.
pub(super) enum StartTurnBusyError {
    /// `TurnAlreadyRunning` persisted after every retry. The consumed
    /// `TurnStartParams` are handed back so the caller can re-enqueue the
    /// prompt instead of silently dropping the user's submit.
    StillBusy(Box<coco_types::TurnStartParams>),
    /// A non-retryable turn/start failure.
    Failed(coco_app_server_client::ClientError),
}

/// detect the local AppServer `TurnAlreadyRunning` handler error. Local
/// dispatch collapses the handler's `HandlerResult::Err` into
/// `ClientError::Server { code, message }`, so match the stable
/// `INVALID_REQUEST` code plus the handler's message. A typed domain kind
/// would be cleaner once exposed by the shared AppServer host boundary.
pub(super) fn is_turn_already_running(error: &coco_app_server_client::ClientError) -> bool {
    matches!(
        error,
        coco_app_server_client::ClientError::Server { code, message, .. }
            if *code == coco_types::error_codes::INVALID_REQUEST
                && message.contains("turn is already running")
    )
}

/// Start a local AppServer turn, retrying a bounded number of times while the
/// handler reports `TurnAlreadyRunning`. On terminal `TurnAlreadyRunning`
/// the params are returned so the caller can re-enqueue rather than drop the
/// submit.
pub(super) async fn start_turn_with_busy_retry(
    bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    session_id: &coco_types::SessionId,
    params: coco_types::TurnStartParams,
) -> Result<coco_types::TurnStartResult, StartTurnBusyError> {
    for attempt in 0..START_TURN_BUSY_RETRIES {
        match bridge.start_turn(session_id.clone(), params.clone()).await {
            Ok(result) => return Ok(result),
            Err(error) if is_turn_already_running(&error) => {
                if attempt + 1 < START_TURN_BUSY_RETRIES {
                    tokio::time::sleep(START_TURN_BUSY_BACKOFF).await;
                }
            }
            Err(error) => return Err(StartTurnBusyError::Failed(error)),
        }
    }
    Err(StartTurnBusyError::StillBusy(Box::new(params)))
}

pub(super) async fn spawn_history_turn_through_app_server(
    messages: Vec<std::sync::Arc<coco_messages::Message>>,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    let session_id = session.session_id().clone();
    let history_override = match messages
        .iter()
        .map(|message| serde_json::to_value(message.as_ref()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(history_override) => history_override,
        Err(error) => {
            tracing::warn!(%error, "history turn AppServer serialization failed");
            return;
        }
    };

    if let Err(error) = local_app_server_bridge
        .activate_existing_interactive_session(session_id.clone(), Some(event_tx.clone()))
    {
        tracing::warn!(%error, "history turn could not activate local AppServer session");
        return;
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    let passive_surface = match monitor_client.attach_passive_session(session_id.clone()) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "history turn could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        target: interactive_target(local_app_server_bridge),
        prompt: String::new(),
        history_override,
        images: Vec::new(),
        composer: Default::default(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
        goal_continuation: false,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "history turn AppServer turn/start failed");
            return;
        }
    };

    let turn_id = uuid::Uuid::new_v4();
    let turn_done_tx_t = turn_done_tx.clone();
    let protocol_turn_id = started.turn_id.clone();
    let interrupt_client = local_app_server_bridge.connect_local_client();
    let handler = local_app_server_bridge.handler().clone();
    let interrupt_target = interactive_target(local_app_server_bridge);
    let task = tokio::spawn(async move {
        let _done = TurnDoneGuard {
            turn_id,
            tx: turn_done_tx_t,
        };
        while let Some(envelope) = monitor_client.next_passive_event(&passive_surface).await {
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
                && ended.turn_id == protocol_turn_id
            {
                break;
            }
        }
    });
    *active_turn.lock().await = Some(ActiveTurn {
        id: turn_id,
        task,
        cancel: ActiveTurnCancel {
            client: interrupt_client,
            handler,
            target: interrupt_target,
        },
    });
}

/// Spawn the per-turn engine task for a slash command that expanded
/// to a model prompt (`SlashFollowup::RunEngine`). Used by the
/// command-palette + SDK invocation paths; the typed-input path
/// substitutes `effective_content` instead so it keeps the outer
/// `user_message_id` from the original TUI submit.
/// The active-turn slot is installed inline (locking `active_turn`)
/// before this returns — callers can immediately start observing
/// `ActiveTurn` from a peer task without a TOCTOU window.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_slash_run_engine_turn(
    prompt: SlashEnginePrompt,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    session_id: &coco_types::SessionId,
) {
    let SlashEnginePrompt {
        content,
        metadata,
        thinking_level,
        model_runtime_source,
    } = prompt;
    if let Err(error) = local_app_server_bridge
        .activate_existing_interactive_session(session_id.clone(), Some(event_tx.clone()))
    {
        tracing::warn!(%error, "slash RunEngine could not activate local AppServer session");
        return;
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    // live-only tail attach with no replay cursor (see SubmitInput site).
    let passive_surface = match monitor_client.attach_passive_session(session_id.clone()) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "slash RunEngine could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        target: interactive_target(local_app_server_bridge),
        prompt: content,
        history_override: Vec::new(),
        images: Vec::new(),
        composer: Default::default(),
        slash_metadata: metadata,
        model_selection: model_runtime_source_to_turn_start_selection(model_runtime_source),
        permission_mode: None,
        thinking_level,
        goal_continuation: false,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "slash RunEngine AppServer turn/start failed");
            return;
        }
    };
    let turn_id = uuid::Uuid::new_v4();
    let session_t = session.clone();
    let title_gen_attempted_t = title_gen_attempted.clone();
    let turn_done_tx_t = turn_done_tx.clone();
    let session_id_t = session_id.clone();
    let protocol_turn_id = started.turn_id.clone();
    let auto_title_client = local_app_server_bridge.connect_local_client();
    let auto_title_handler = local_app_server_bridge.handler().clone();
    let task = tokio::spawn(async move {
        let _done = TurnDoneGuard {
            turn_id,
            tx: turn_done_tx_t,
        };
        let mut auto_title_client = Some(auto_title_client);
        let mut auto_title_handler = Some(auto_title_handler);
        while let Some(envelope) = monitor_client.next_passive_event(&passive_surface).await {
            if let CoreEvent::Protocol(ServerNotification::TurnEnded(ended)) = envelope.event
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
            target: interactive_target(local_app_server_bridge),
        },
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_slash_command(
    name: &str,
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    images: &[coco_types::QueuedCommandEditImage],
) -> SlashOutcome {
    let runtime = session;
    let session_id = session.session_id();
    let side_chat = local_app_server_bridge.is_child_session(session_id);
    let canonical_name = if side_chat {
        let registry = session.current_command_registry().await;
        let Some(command) = registry.get(name).filter(|command| {
            command.is_active() && command.base.session_scope.supports_side_chat()
        }) else {
            emit_slash_text(
                event_tx,
                session_id,
                name,
                args,
                SIDECHAT_SLASH_POLICY_MESSAGE,
            )
            .await;
            return SlashOutcome::Handled;
        };
        Some(command.base.name.clone())
    } else {
        None
    };
    let name = canonical_name.as_deref().unwrap_or(name);
    if name == "btw" {
        return SlashOutcome::TriggerBtw {
            request: coco_commands::handlers::btw::BtwRequest::parse(args),
            images: images.to_vec(),
        };
    }
    // Runtime-state-aware commands intercepted before registry lookup:
    // their behavior depends on per-session state (session_id, plan
    // file, app_state) that the static registry can't carry.
    if matches!(name, "plan" | "planning") {
        return dispatch_plan(args, session, event_tx).await;
    }
    // `/permissions` (no arg) / `/permissions list` — open the tabbed
    // rule-editor overlay. The subcommand
    // forms (`allow` / `deny` / `reset`) keep their session-mutation
    // behavior below for power users + SDK parity.
    if name == "permissions" && matches!(args.trim(), "" | "list") {
        let payload =
            coco_agent_host::session_dialogs::build_permissions_editor_payload(session).await;
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenPermissionsEditor {
                payload,
            }))
            .await;
        return SlashOutcome::Handled;
    }
    // `/permissions allow|deny|reset` — the registry handler can't
    // mutate the live `ToolAppState.permissions` base. Intercept the
    // mutating subcommands so they actually take effect.
    if name == "permissions"
        && let Some(outcome) =
            dispatch_permissions_mutation(args, session, event_tx, local_app_server_bridge).await
    {
        return outcome;
    }
    // `/color <name|default>` mutates session app state through local AppServer.
    // The registry handler is sync + has no runtime context, so the intercept
    // owns the teammate guard. Falls through to the registry (handler lists
    // colors) when args are empty.
    if name == "color"
        && let Some(outcome) =
            dispatch_color(args, session, event_tx, local_app_server_bridge).await
    {
        return outcome;
    }
    // `/clear` mutates runtime state. Keep it in the command layer so
    // typed and palette dispatch both run the real clear flow instead
    // of letting a registry text handler print without clearing.
    // Resolve aliases (`/reset`, `/new`) to the canonical `clear` name
    // first so they trigger the same flow instead of falling through to
    // the generic registry handler (`clear` declares aliases `['reset', 'new']`).
    let resolves_to_clear =
        coco_agent_host::session_controls::command_resolves_to(session, name, "clear").await;
    if name == "clear" || resolves_to_clear {
        return SlashOutcome::TriggerClear;
    }
    if name == "context" {
        return dispatch_context(session, event_tx, local_app_server_bridge).await;
    }
    // `/config` (alias `/settings`) with no args opens the interactive settings
    // panel, reusing the same overlay as the `Ctrl+,` keybind. `config <key>
    // <value>` still falls through to the
    // registry text handler that writes settings.json.
    if matches!(name, "config" | "settings") && args.trim().is_empty() {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::OpenSettings))
            .await;
        return SlashOutcome::Handled;
    }
    if name == "add-dir" {
        if args.trim().is_empty() {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenAddDirectory))
                .await;
            return SlashOutcome::Handled;
        }
        return dispatch_add_dir(args, session, event_tx, local_app_server_bridge).await;
    }
    // `/export` (no arg) opens the Markdown/JSON/Text format picker;
    // `/export <format>` renders the live conversation history in that format
    // and writes it to a file in the session's original cwd. The sync registry
    // handler has no runtime access (can't reach `MessageHistory`), so the real
    // export lives here..
    if name == "export" {
        if args.trim().is_empty() {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenExport))
                .await;
            return SlashOutcome::Handled;
        }
        return run_export(args, session, event_tx).await;
    }
    // `/branch` (alias `/fork`) forks the conversation at this point into a new
    // session and switches to it live. The sync registry handler only echoes
    // text — the real fork needs runtime + session-store access.
    if matches!(name, "branch" | "fork") {
        return dispatch_branch(
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_reload_subscriptions,
        )
        .await;
    }
    if name == "resume" {
        return dispatch_resume(
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_reload_subscriptions,
        )
        .await;
    }
    // `/copy [N]` — the picker + arg-parsing + lookback logic lives in
    // the TUI (only it owns the transcript view); the dispatcher just
    // hands off the raw args.
    if name == "copy" {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::CopyCommandRequested {
                args: args.to_string(),
            }))
            .await;
        return SlashOutcome::Handled;
    }
    // `/login [provider]` / `/logout [provider]` activate a configured OAuth
    // subscription against the SHARED `AuthService`, so the running session's
    // clients pick up the new token immediately. Handled here (not the
    // registry) because the auth flow lives in `app/cli` + needs the runtime.
    if name == "login" {
        // No-arg `/login` opens the provider picker; `/login <provider>` logs
        // in directly.
        if args.trim().is_empty() {
            let entries = coco_agent_host::session_dialogs::build_login_entries_payload(session);
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenLoginPicker { entries }))
                .await;
            return SlashOutcome::Handled;
        }
        return dispatch_provider_login(args, session, event_tx).await;
    }
    if name == "logout" {
        return dispatch_provider_logout(args, session, event_tx).await;
    }
    if name == "model"
        && !args.trim().is_empty()
        && let Some(restricted) =
            coco_agent_host::session_controls::restricted_model_selection_for_args(session, args)
    {
        let full_model_name = format!("{}/{}", restricted.provider, restricted.model_id);
        emit_slash_text(
            event_tx,
            session.session_id(),
            "model",
            args,
            &format!(
                "Model '{full_model_name}' is restricted by your organization's settings. Run /model to choose a different model.",
            ),
        )
        .await;
        return SlashOutcome::Handled;
    }
    // `/rewind` flows through the standard registry → handler →
    // `DialogSpec::MessageSelector` → `OpenRewindPicker` path. The
    // handler ignores args; this dispatcher does only the
    // mechanical translation in the generic `DialogSpec` arm below.
    if name == "tasks" || name == "bashes" {
        run_tasks_command(session, event_tx, local_app_server_bridge, name, args).await;
        return SlashOutcome::Handled;
    }
    if name == "diff" && matches!(args.split_whitespace().next(), Some("session" | "turn")) {
        run_file_history_diff_command(session, event_tx, args).await;
        return SlashOutcome::Handled;
    }

    let cmd = match coco_agent_host::session_slash::resolve_registered_command(session, name).await
    {
        coco_agent_host::session_slash::ResolvedSlashCommand::NotFound
        | coco_agent_host::session_slash::ResolvedSlashCommand::PromptWithoutHandler => {
            return SlashOutcome::NotFound;
        }
        coco_agent_host::session_slash::ResolvedSlashCommand::Inactive => {
            let text = slash_unavailable_in_session_message(name);
            emit_slash_text(event_tx, session.session_id(), name, args, &text).await;
            return SlashOutcome::Handled;
        }
        coco_agent_host::session_slash::ResolvedSlashCommand::Loop { canonical_name } => {
            let prompt =
                coco_agent_host::session_controls::build_loop_command_prompt(session, args).await;
            return SlashOutcome::RunEngine {
                content: prompt,
                metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                    &canonical_name,
                    args,
                )),
                thinking_level: None,
                model_runtime_source: None,
            };
        }
        coco_agent_host::session_slash::ResolvedSlashCommand::ForkSkill { canonical_name } => {
            return SlashOutcome::RunForkSkill {
                name: canonical_name,
                args: args.to_string(),
            };
        }
        coco_agent_host::session_slash::ResolvedSlashCommand::NoHandler => {
            emit_slash_status(
                event_tx,
                session.session_id(),
                name,
                args,
                SlashCommandStatusKind::NoHandler,
            )
            .await;
            return SlashOutcome::Handled;
        }
        coco_agent_host::session_slash::ResolvedSlashCommand::Executable(command) => command,
    };

    let result = match cmd.handler.execute_command(args).await {
        Ok(r) => {
            // Skill lifecycle telemetry for the INLINE slash path. Fork-mode
            // skills returned above (RunForkSkill → QuerySkillRuntime records);
            // the model/SkillTool path records in skill_runtime. This is the
            // only seam that sees a user typing `/name` for an inline skill —
            // the accrual path the Curator promotes/retires on. `handler` is
            // the resolved trait object, so the registry-wrapper record site is
            // never reached in production.
            cmd.record_skill_invocation(coco_skills::telemetry::SkillOutcome::Success);
            r
        }
        Err(e) => {
            cmd.record_skill_invocation(coco_skills::telemetry::SkillOutcome::Failure);
            emit_slash_status(
                event_tx,
                session.session_id(),
                name,
                args,
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
            return SlashOutcome::Handled;
        }
    };

    use coco_commands::{CommandResult, DialogSpec, PromptPart};
    match result {
        CommandResult::Skip => SlashOutcome::Handled,
        CommandResult::TriggerSkillLearn { directive } => {
            // `/learn` — fire a user-initiated review fork now, bypassing the
            // turn throttle. The fork context is this session's history slice;
            // the notice channel announces the result on a later turn.
            let text = match session.skill_review_runtime() {
                Some(runtime) => {
                    let fork_context = session.history_messages().await;
                    match runtime.manual_review(directive, session.session_id(), fork_context) {
                        coco_skill_learn::ReviewTrigger::Spawned => {
                            "Learning from this session — the skill will be announced when ready."
                        }
                        coco_skill_learn::ReviewTrigger::InProgress => {
                            "A skill review is already running; try again once it finishes."
                        }
                        coco_skill_learn::ReviewTrigger::Skipped
                        | coco_skill_learn::ReviewTrigger::Throttled => {
                            "Skill learning is not available right now."
                        }
                    }
                }
                // The runtime is absent when EITHER gate is closed, so name the
                // one that actually is: sending a user to flip a feature they
                // already enabled is worse than saying nothing.
                None => {
                    let cfg = session.runtime_config();
                    if !cfg.features.enabled(coco_types::Feature::SkillLearning) {
                        "Skill learning is off — set `features.skill_learning: true` in settings.json."
                    } else if !cfg.skill_learn.enabled {
                        "Skill learning is off — set `skill_learn.enabled: true` in settings.json."
                    } else {
                        "Skill learning is unavailable in this session."
                    }
                }
            };
            emit_slash_text(event_tx, session.session_id(), name, args, text).await;
            SlashOutcome::Handled
        }
        CommandResult::Text(text) => {
            // Sentinel detection — handlers like `/compact`, `/dream`,
            // `/summary` produce a sentinel-prefixed string instead of
            // having direct access to the runtime. Convert the sentinel
            // into a structured `SlashOutcome` so the agent driver runs
            // the real feature (compaction, consolidation, extraction).
            // Mirrors the SDK turn/start sentinel detection for the
            // non-interactive path.
            if let Some(trigger) = classify_sentinel_trigger(&text) {
                return match trigger {
                    SentinelTrigger::Compact {
                        custom_instructions,
                    } => SlashOutcome::TriggerCompact {
                        custom_instructions,
                    },
                    SentinelTrigger::Dream => SlashOutcome::TriggerDream,
                    SentinelTrigger::Summary => SlashOutcome::TriggerSummary,
                    SentinelTrigger::Cost => SlashOutcome::ShowCost,
                    SentinelTrigger::Status => SlashOutcome::ShowStatus,
                    SentinelTrigger::Goal { request } => SlashOutcome::TriggerGoal { request },
                    SentinelTrigger::Rename { request } => SlashOutcome::TriggerRename { request },
                    SentinelTrigger::Tag { tag } => SlashOutcome::TriggerTag { tag },
                    SentinelTrigger::AddDir { path } => SlashOutcome::TriggerAddDir { path },
                    SentinelTrigger::ReloadPlugins => SlashOutcome::TriggerReloadPlugins,
                    SentinelTrigger::ReloadHooks => SlashOutcome::TriggerReloadHooks,
                };
            }
            emit_slash_text(event_tx, session.session_id(), name, args, &text).await;
            SlashOutcome::Handled
        }
        CommandResult::InjectPrompt(text) => SlashOutcome::RunEngine {
            content: text,
            metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                &cmd.canonical_name,
                args,
            )),
            thinking_level: None,
            model_runtime_source: None,
        },
        CommandResult::MoaOneShot { prompt } => SlashOutcome::RunEngine {
            content: prompt,
            metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                &cmd.canonical_name,
                args,
            )),
            thinking_level: None,
            model_runtime_source: Some(
                coco_agent_host::session_controls::moa_one_shot_model_runtime_source(session),
            ),
        },
        CommandResult::Prompt { parts, .. } => {
            // Concatenate text parts. `File` parts are not yet wired —
            // none of the in-tree Prompt handlers emit them today.
            let mut buf = String::new();
            for part in parts {
                match part {
                    PromptPart::Text { text } => {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(&text);
                    }
                    PromptPart::File { .. } => {
                        warn!(%name, "Prompt::File parts not yet rendered to engine input");
                    }
                }
            }
            if buf.is_empty() {
                emit_slash_status(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    SlashCommandStatusKind::EmptyPrompt,
                )
                .await;
                SlashOutcome::Handled
            } else {
                SlashOutcome::RunEngine {
                    content: buf,
                    metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                        &cmd.canonical_name,
                        args,
                    )),
                    thinking_level: match &cmd.command_type {
                        coco_types::CommandType::Prompt(data) => data.thinking_level.clone(),
                        _ => None,
                    },
                    model_runtime_source: None,
                }
            }
        }
        CommandResult::Compact {
            display_text,
            summary,
        } => {
            // Pre-computed summary path: a handler that already ran
            // compaction (or has a summary in hand) returns the summary
            // string + display text. We push the summary as a
            // `is_compact_summary: true` user message so the next turn
            // sees it as a compact boundary; the LLM-summarized engine
            // path is unchanged (it's still the entry-point for typed
            // `/compact` from the TUI fast-path).
            // Truncation of pre-summary rounds is intentionally left to
            // the handler — when no handler emits this today, we err on
            // the side of preserving history rather than dropping it.
            coco_agent_host::session_messages::append_compact_summary_to_history_and_emit(
                runtime,
                event_tx.clone(),
                &summary,
            )
            .await;
            emit_slash_text(event_tx, session.session_id(), name, args, &display_text).await;
            SlashOutcome::Handled
        }
        CommandResult::OpenDialog(spec) => {
            // Wired dialogs route to TuiOnlyEvent so the TUI opens the
            // modal; unwired dialogs emit a localized breadcrumb.
            match spec {
                DialogSpec::MessageSelector => {
                    tracing::debug!(
                        target: "rewind::dispatch",
                        "translating DialogSpec::MessageSelector → OpenRewindPicker",
                    );
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenRewindPicker))
                        .await;
                }
                DialogSpec::MemoryFileSelector { entries } => {
                    // Convert from coco_commands::MemoryFileEntry to the
                    // wire-payload struct in coco-types so the TUI can
                    // consume the event without depending on coco-commands.
                    let wire_entries: Vec<coco_types::MemoryDialogEntry> = entries
                        .into_iter()
                        .map(|e| {
                            let row_kind = if e.is_folder {
                                coco_types::MemoryDialogRowKind::Folder { enabled: true }
                            } else {
                                coco_types::MemoryDialogRowKind::File {
                                    exists: !e.is_new,
                                    read_only: false,
                                }
                            };
                            coco_types::MemoryDialogEntry {
                                path: e.path.display().to_string(),
                                label: e.label,
                                scope: match e.scope {
                                    coco_commands::MemoryScope::Managed => {
                                        coco_types::MemoryDialogScope::Managed
                                    }
                                    coco_commands::MemoryScope::User => {
                                        coco_types::MemoryDialogScope::User
                                    }
                                    coco_commands::MemoryScope::Project => {
                                        coco_types::MemoryDialogScope::Project
                                    }
                                    coco_commands::MemoryScope::ProjectLocal => {
                                        coco_types::MemoryDialogScope::ProjectLocal
                                    }
                                    coco_commands::MemoryScope::ProjectConfig => {
                                        coco_types::MemoryDialogScope::ProjectConfig
                                    }
                                    coco_commands::MemoryScope::Subdir => {
                                        coco_types::MemoryDialogScope::Subdir
                                    }
                                    coco_commands::MemoryScope::Imported => {
                                        coco_types::MemoryDialogScope::Imported
                                    }
                                    coco_commands::MemoryScope::AutoMemFolder => {
                                        coco_types::MemoryDialogScope::AutoMemFolder
                                    }
                                    coco_commands::MemoryScope::TeamMemFolder => {
                                        coco_types::MemoryDialogScope::TeamMemFolder
                                    }
                                    coco_commands::MemoryScope::AgentMemFolder => {
                                        coco_types::MemoryDialogScope::AgentMemFolder
                                    }
                                },
                                row_kind,
                            }
                        })
                        .collect();
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenMemoryDialog {
                            entries: wire_entries,
                        }))
                        .await;
                }
                DialogSpec::ModelPicker => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenModelPicker))
                        .await;
                }
                DialogSpec::ProviderWizard => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenProviderWizard))
                        .await;
                }
                DialogSpec::ThemePicker => {
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenThemePicker))
                        .await;
                }
                DialogSpec::WorkflowPicker => {
                    let payload =
                        coco_agent_host::session_dialogs::build_workflow_dialog_payload(runtime)
                            .await;
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenWorkflowPicker { payload }))
                        .await;
                }
                DialogSpec::SkillsList { mut payload } => {
                    coco_agent_host::session_dialogs::enrich_skills_dialog_payload(
                        runtime,
                        &mut payload,
                    )
                    .await;
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenSkillsDialog { payload }))
                        .await;
                }
                DialogSpec::AgentsList { payload } => {
                    // The handler ships the agent catalog as it
                    // looks on disk; running counts are derived TUI-
                    // side from the live `SessionState.subagents`
                    // mirror, so no enrichment is needed here. Mid-
                    // session edits route through host agent operations
                    // and a fresh payload round-trip rather than
                    // mutating in place.
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenAgentsDialog { payload }))
                        .await;
                }
                DialogSpec::PluginPicker => {
                    refresh_plugin_dialog_payload(session, event_tx).await;
                }
                DialogSpec::Journey => {
                    // Assemble the learning timeline host-side (blocking disk
                    // walks run on a blocking thread inside the builder), then
                    // hand the wire snapshot to the TUI overlay.
                    let payload =
                        coco_agent_host::session_dialogs::build_journey_dialog_payload(session)
                            .await;
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::OpenJourneyDialog { payload }))
                        .await;
                }
                DialogSpec::McpbConfig { .. } | DialogSpec::Confirm { .. } => {
                    let dialog_kind = match spec {
                        DialogSpec::McpbConfig { .. } => "MCPB config form",
                        DialogSpec::Confirm { .. } => "confirm dialog",
                        DialogSpec::MessageSelector
                        | DialogSpec::MemoryFileSelector { .. }
                        | DialogSpec::SkillsList { .. }
                        | DialogSpec::AgentsList { .. }
                        | DialogSpec::PluginPicker
                        | DialogSpec::Journey
                        | DialogSpec::ModelPicker
                        | DialogSpec::ProviderWizard
                        | DialogSpec::WorkflowPicker
                        | DialogSpec::ThemePicker => unreachable!(),
                    }
                    .to_string();
                    emit_slash_status(
                        event_tx,
                        session.session_id(),
                        name,
                        args,
                        SlashCommandStatusKind::DialogPending { dialog_kind },
                    )
                    .await;
                }
            }
            SlashOutcome::Handled
        }
    }
}

pub(super) const MAX_FILE_HISTORY_DIFF_CHARS: usize = 6000;

/// description and show the plan.
pub(super) async fn dispatch_plan(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let runtime = session;
    let args = args.trim();

    // Plan mode opted out via `features.plan_mode = false`: don't flip into
    // Plan, just tell the user. Mirrors the hidden plan-mode tools, the
    // suppressed reminders, and the Plan rung removed from the Shift+Tab cycle.
    if !coco_agent_host::session_controls::plan_mode_feature_enabled(session) {
        emit_slash_text(
            event_tx,
            session.session_id(),
            "plan",
            args,
            "Plan mode is disabled (`features.plan_mode = false`). \
             Re-enable it in settings.json to use `/plan`.",
        )
        .await;
        return SlashOutcome::Handled;
    }

    let session_id = runtime.session_id().clone();
    let plans_dir = runtime.configured_plans_dir();

    // Flip state for ALL `/plan` invocations when not already in plan
    // mode — bare `/plan`, `/plan open`, and `/plan <description>` all
    // consent to plan mode equally.
    let plan_mode =
        coco_agent_host::live_permission_mode::ensure_plan_mode(session, event_tx).await;
    let was_in_plan = plan_mode.previous == coco_types::PermissionMode::Plan;
    if plan_mode.changed {
        info!(
            session_id = %plan_mode.session_id,
            from = ?plan_mode.previous,
            to = ?coco_types::PermissionMode::Plan,
            "TUI /plan: direct-toggle to Plan mode",
        );
    }

    // Path to the (resolved) session plan file — used by every arm.
    let plan_path =
        coco_context::get_plan_file_path(session_id.as_str(), &plans_dir, /*agent_id*/ None);

    if args.is_empty() {
        let content =
            coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
        let body = match content {
            Some(body) if !body.trim().is_empty() => format!(
                "## Current Plan\n\n*{}*\n\n{}\n\nRun `/plan open` to edit in $EDITOR.",
                plan_path.display(),
                body
            ),
            _ => format!(
                "No plan written yet for this session.\n\n\
                 Plan file: `{}`\n\n\
                 Run `/plan <description>` to plan for a task in plan mode, \
                 or `/plan open` to start an empty plan in $EDITOR.",
                plan_path.display()
            ),
        };
        let text = if was_in_plan {
            body
        } else {
            format!("Enabled plan mode.\n\n{body}")
        };
        emit_slash_text(event_tx, session.session_id(), "plan", args, &text).await;
        return SlashOutcome::Handled;
    }

    if args == "open" {
        let text = if was_in_plan {
            format!("Opening plan file: {}", plan_path.display())
        } else {
            format!(
                "Enabled plan mode.\n\nOpening plan file: {}",
                plan_path.display()
            )
        };
        emit_slash_text(event_tx, session.session_id(), "plan", args, &text).await;
        return SlashOutcome::TriggerOpenPlanEditor { path: plan_path };
    }

    // `/plan <description>` —
    // - Flipped to plan mode → fire query with the user input.
    // Returns `RunEngine { content: <description> }`.
    // - Already in plan mode → ignore the description, just show the plan.
    if was_in_plan {
        let content =
            coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
        let text = match content {
            Some(body) if !body.trim().is_empty() => format!(
                "Already in plan mode.\n\n## Current Plan\n\n*{}*\n\n{}\n\n\
                 Run `/plan open` to edit in $EDITOR.",
                plan_path.display(),
                body
            ),
            _ => "Already in plan mode. No plan written yet.".to_string(),
        };
        emit_slash_text(event_tx, session.session_id(), "plan", args, &text).await;
        return SlashOutcome::Handled;
    }
    match plan_command_query_after_flip(args) {
        Some(desc) => SlashOutcome::RunEngine {
            content: desc.to_string(),
            metadata: Some(coco_agent_host::session_messages::slash_command_metadata(
                "plan", args,
            )),
            thinking_level: None,
            model_runtime_source: None,
        },
        None => {
            // Unreachable in practice — bare `/plan` and `/plan open`
            // are handled by the earlier branches. Kept defensive so
            // future edits to the cascade can't silently fall through.
            SlashOutcome::Handled
        }
    }
}
