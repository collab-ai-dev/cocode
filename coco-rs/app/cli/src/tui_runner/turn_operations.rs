use super::*;

/// In-flight turn handle. Each `SubmitInput` / `ExecuteSkill` spawns
/// the engine call into a child task so the `command_rx` recv loop stays
/// responsive (Interrupt / ClearConversation / Compact / Rewind / Shutdown
/// can reach their arms while the engine runs). Rust's explicit
/// `tokio::spawn` keeps the recv loop unblocked.
pub(super) struct ActiveTurn {
    pub(super) id: uuid::Uuid,
    pub(super) task: tokio::task::JoinHandle<()>,
    pub(super) cancel: ActiveTurnCancel,
}

pub(super) struct ActiveTurnCancel {
    /// AppServer-owned turn: cancellation flows through the same
    /// `turn/interrupt` request the SDK uses.
    pub(super) client: coco_agent_host::local_client::LocalServerClient<
        coco_agent_host::sdk_server::LocalAppSessionHandle,
    >,
    pub(super) handler: coco_agent_host::sdk_server::AppServerSdkHandler,
}

/// Always-fires completion signaller for spawned turn tasks.
/// The main `select!` loop in `run_agent_driver` blocks on
/// `turn_done_rx.recv()` to drain a completed turn from `active_turn`.
/// Sending `turn_id` as the last statement of the spawned task only
/// covers the happy path: a panic inside a spawned turn body unwinds
/// before reaching the send, so the `active_turn` slot stays occupied
/// with a corpse `JoinHandle` until the next user command forces
/// `drain_active_turn()` to collect it.
/// `Drop` runs on both normal scope-exit and panic unwind. `try_send`
/// is non-blocking and safe in `Drop`; the receiver is drained promptly
/// so the bounded channel (buffer 16) should never be full in practice.
pub(super) struct TurnDoneGuard {
    pub(super) turn_id: uuid::Uuid,
    pub(super) tx: mpsc::Sender<uuid::Uuid>,
}

impl Drop for TurnDoneGuard {
    fn drop(&mut self) {
        if let Err(err) = self.tx.try_send(self.turn_id) {
            warn!(
                turn_id = %self.turn_id,
                error = ?err,
                "turn completion signal failed in Drop; active_turn may stay locked until next drain"
            );
        }
    }
}

/// Completion signaller for the cross-process teammate inbox pump (gap 1).
/// Fires the turn's `user_message_id` so the pump (`teammate_inbox_pump`)
/// can release its serialized wait and inject the next mailbox message.
/// `Drop` (not a tail send) so the signal fires on normal completion,
/// cancellation, AND panic — same reasoning as [`TurnDoneGuard`]. Only
/// attached in a teammate session (the pump is the sole consumer); the
/// `try_send` is best-effort against the bounded handshake channel.
pub(super) struct PumpDoneGuard {
    pub(super) id: String,
    pub(super) tx: mpsc::Sender<String>,
}

impl Drop for PumpDoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.try_send(self.id.clone());
    }
}

pub(super) const TUI_SHUTDOWN_ACTIVE_TURN_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveTurnDrain {
    Wait,
    AbortAfter(Duration),
}

pub(super) enum PendingEditorRequest {
    Memory {
        path: std::path::PathBuf,
    },
    Plan {
        path: std::path::PathBuf,
    },
    PlanPrompt {
        request_id: String,
        initial_content: String,
        path: Option<std::path::PathBuf>,
    },
    Prompt {
        initial_content: String,
    },
    /// `/agents` Library tab Enter on an editable agent row → fork
    /// `$EDITOR` against the markdown source path. On editor exit the
    /// runner re-reads the agent catalog and re-emits the dialog
    /// payload so the dialog refreshes against the new on-disk state.
    Agent {
        path: std::path::PathBuf,
    },
}

/// Cancel the in-flight turn (if any) and drain its task.
/// Used by every arm whose semantics conflict with a concurrent
/// turn (Clear / Compact / Rewind / Shutdown / next SubmitInput).
/// `AbortAfter` is reserved for explicit process shutdown so a stuck
/// tool or stream cannot leave the terminal sitting on the exit hint.
/// Cancellation now goes through AppServer `turn/interrupt`; the server-side
/// runner owns the terminal `TurnEnded` emission and reason mapping.
pub(super) async fn drain_active_turn(
    slot: &Arc<Mutex<Option<ActiveTurn>>>,
    mode: ActiveTurnDrain,
) {
    let state = { slot.lock().await.take() };
    if let Some(s) = state {
        let ActiveTurnCancel { client, handler } = &s.cancel;
        if let Err(error) = client.turn_interrupt(handler).await {
            tracing::warn!(%error, "drain_active_turn: AppServer turn/interrupt failed");
        }
        match mode {
            ActiveTurnDrain::Wait => {
                let _ = s.task.await;
            }
            ActiveTurnDrain::AbortAfter(timeout) => {
                let mut task = s.task;
                tokio::select! {
                    result = &mut task => {
                        let _ = result;
                    }
                    _ = tokio::time::sleep(timeout) => {
                        warn!(
                            timeout_ms = timeout.as_millis(),
                            "active turn did not stop during TUI shutdown; aborting task"
                        );
                        task.abort();
                        let _ = task.await;
                    }
                }
            }
        }
    }
}

pub(super) async fn drain_completed_turn(
    slot: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_id: uuid::Uuid,
) -> bool {
    let state = {
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|s| s.id == turn_id) {
            guard.take()
        } else {
            None
        }
    };
    if let Some(s) = state {
        let _ = s.task.await;
        true
    } else {
        false
    }
}

/// Run a manual full LLM compaction. Used by `UserCommand::Compact` and
/// the slash dispatcher's `TriggerCompact` outcome — both routes feed
/// through here so typed `/compact` and palette `/compact` behave
/// identically.
pub(super) async fn run_manual_compact(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    custom_instructions: Option<String>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    // Drain any active turn before compacting: the AppServer compact shortcut
    // mutates the active history and runs an LLM call.
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt =
        match coco_commands::handlers::compact::handler(custom_instructions.unwrap_or_default())
            .await
        {
            Ok(prompt) => prompt,
            Err(error) => {
                warn!(%error, "TUI /compact handler failed");
                return;
            }
        };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /compact",
    )
    .await;
}

pub(super) async fn run_local_app_server_shortcut_turn(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    prompt: String,
    log_label: &'static str,
) {
    let session_id = session.current_typed_session_id().await;
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(session_id.clone(), event_tx.clone())
    {
        tracing::warn!(%error, "{log_label} could not refresh local AppServer event pump");
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    // live-only tail attach with no replay cursor (see SubmitInput site).
    let passive_surface = match monitor_client.attach_passive_session(session_id.clone()) {
        Ok(surface) => surface,
        Err(error) => {
            tracing::warn!(%error, "{log_label} could not attach AppServer completion monitor");
            return;
        }
    };
    let params = coco_types::TurnStartParams {
        prompt,
        history_override: Vec::new(),
        images: Vec::new(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
    };
    let started = match local_app_server_bridge
        .start_turn(session_id.clone(), params)
        .await
    {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "{log_label} AppServer turn/start failed");
            return;
        }
    };
    let turn_id = uuid::Uuid::new_v4();
    let protocol_turn_id = started.turn_id.clone();
    let turn_done_tx_t = turn_done_tx.clone();
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
    let interrupt_client = local_app_server_bridge.connect_local_client();
    let handler = local_app_server_bridge.handler().clone();
    *active_turn.lock().await = Some(ActiveTurn {
        id: turn_id,
        task,
        cancel: ActiveTurnCancel {
            client: interrupt_client,
            handler,
        },
    });
}

/// Run a user-typed fork-mode skill (`/<name>` with `context: fork`) as a
/// subagent and inject its result into the transcript.
/// `executeForkedSlashCommand`: the subagent runs synchronously, its final
/// text lands as a `<local-command-stdout>` user message, and there is NO
/// follow-up main-model query.
/// Drains the in-flight turn first (the subagent runs LLM calls / mutates
/// shared state) — same contract as `run_manual_compact`.
pub(super) async fn run_fork_skill(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    name: &str,
    args: &str,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
) {
    let runtime = session;
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;

    let body = match runtime.invoke_skill_fork(name, args).await {
        Ok(output) => format!("<local-command-stdout>\n{output}\n</local-command-stdout>"),
        Err(e) => {
            warn!(skill = %name, error = %e, "fork-mode skill failed");
            format!("<local-command-stderr>\nSkill '/{name}' failed: {e}\n</local-command-stderr>")
        }
    };
    // Persist the command marker + result via history_push_and_emit so the
    // TUI transcript renders them and the next turn's model sees what ran.
    let mut h = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    coco_query::history_sync::history_push_and_emit(
        &mut h,
        create_slash_metadata_message(&format_slash_command_metadata(name, args)),
        &event_tx_opt,
    )
    .await;
    coco_query::history_sync::history_push_and_emit(
        &mut h,
        coco_messages::create_user_message(&body),
        &event_tx_opt,
    )
    .await;
}

/// Run the clear flow. Drains any active turn first since clear mutates
/// session_id + resets several per-session caches.
/// Plan I-1 (Authority): emits a wire-visible event after the clear so
/// the TUI's `TranscriptView` and SDK NDJSON observers stay coherent.
/// `/clear` rotates session_id → emit
/// `SessionResetForResume { session_id: new }`.
pub(super) async fn run_clear_conversation(
    session: &crate::session_runtime::SessionHandle,
    control_context: &LocalRuntimeControlContext<'_>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let old_session_id = runtime.current_typed_session_id().await;
    if let Err(error) = local_app_server_bridge.ensure_interactive_surface(old_session_id.clone()) {
        warn!(
            %error,
            session_id = %old_session_id,
            "/clear could not confirm local AppServer interactive surface"
        );
        return;
    }
    let permissions = runtime.app_state().read().await.permissions.clone();
    let rewind_messages = runtime.prepare_for_clear_replacement().await;
    let new_session_id = coco_types::SessionId::generate();
    let make_runtime_factory = {
        let runtime_factory = control_context.runtime_factory.clone();
        let process_runtime = Arc::clone(control_context.process_runtime);
        let cwd = control_context.cwd.to_path_buf();
        let event_tx = event_tx.clone();
        let new_session_id = new_session_id.clone();
        let permissions = permissions.clone();
        let rewind_messages = rewind_messages.clone();
        async move {
            build_runtime_for_clear(
                runtime_factory,
                new_session_id,
                permissions,
                rewind_messages,
                process_runtime,
                cwd,
                event_tx,
            )
            .await
        }
    };
    let new_session = match local_app_server_bridge
        .replace_session_runtime_for_clear(
            old_session_id.clone(),
            new_session_id.clone(),
            make_runtime_factory,
        )
        .await
    {
        Ok(Some((session, _surface_id))) => session,
        Ok(None) => {
            warn!(
                session_id = %old_session_id,
                "/clear could not find local AppServer calling surface"
            );
            return;
        }
        Err(error) => {
            warn!(%error, "/clear failed to build replacement runtime");
            return;
        }
    };
    new_session.fire_session_start_hooks("clear").await;
    local_app_server_bridge
        .install_session_runtime(new_session.clone())
        .await;
    {
        let mut current = control_context.current_session.write().await;
        *current = new_session.clone();
    }
    control_context
        .runtime_reload_subscriptions
        .lock()
        .await
        .install_for_session(&new_session)
        .await;
    if let Err(error) = local_app_server_bridge.ensure_interactive_surface(new_session_id.clone()) {
        warn!(
            %error,
            session_id = %new_session_id,
            "/clear could not attach local AppServer interactive surface"
        );
    }
    if let Err(error) =
        local_app_server_bridge.start_passive_event_pump(new_session_id.clone(), event_tx.clone())
    {
        warn!(
            %error,
            session_id = %new_session_id,
            "/clear could not refresh local AppServer event pump"
        );
    }
    let notif = ServerNotification::SessionResetForResume {
        identity: coco_types::ServerNotificationIdentity::new(Some(new_session_id), None),
    };
    let _ = event_tx.send(CoreEvent::Protocol(notif)).await;
    if let Some(messages) = new_session.pre_clear_rewind_messages().await {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::RewindPreClearSnapshot {
                messages,
            }))
            .await;
    }
}

/// Force auto-memory consolidation through the local AppServer `turn/start`
/// sentinel shortcut, matching SDK behavior.
pub(super) async fn run_dream_consolidation(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = match coco_commands::handlers::dream::handler(String::new()).await {
        Ok(prompt) => prompt,
        Err(error) => {
            warn!(%error, "TUI /dream handler failed");
            return;
        }
    };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /dream",
    )
    .await;
}

/// Force a 9-section session-memory update through the local AppServer
/// `turn/start` sentinel shortcut, matching SDK behavior.
pub(super) async fn run_session_memory_force(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = match coco_commands::handlers::summary::handler(String::new()).await {
        Ok(prompt) => prompt,
        Err(error) => {
            warn!(%error, "TUI /summary handler failed");
            return;
        }
    };
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /summary",
    )
    .await;
}

/// `/btw <question>` runner — routes the existing sentinel through local
/// AppServer `turn/start`, matching SDK behavior and keeping the fork+answer
/// logic in the handler shortcut. The shortcut appends model-invisible slash
/// messages and emits a synthetic turn lifecycle for the TUI completion
/// monitor.
pub(super) async fn run_side_question(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    request: coco_commands::handlers::btw::BtwRequest,
) {
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let prompt = coco_commands::handlers::btw::handler(&request.question);
    if coco_commands::handlers::btw::parse_btw_sentinel(&prompt).is_none() {
        warn!("TUI /btw handler returned a non-sentinel prompt");
        return;
    }
    run_local_app_server_shortcut_turn(
        session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        turn_done_tx,
        prompt,
        "TUI /btw",
    )
    .await;
}

/// `/export <filename>` runner — renders the live conversation `MessageHistory`
/// (incl. tool activity) and writes it to a file in the session's original cwd,
/// then confirms the path. The sync registry handler has no runtime access, so
/// the real export lives here.: the arg
/// is a FILENAME and the file is written under the cwd. coco infers the format
/// from the extension (`.md`→markdown, `.json`→json, else plain text) — TS
/// exports plain text only. The no-arg format-picker modal re-enters here with
/// a bare format keyword (`markdown`/`json`/`text`), for which a timestamped
/// default filename is generated.(Clipboard export lives in `/copy`.)
pub(super) async fn run_export(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let runtime = session;
    use crate::conversation_export::ExportFormat;
    let arg = args.trim();
    // A bare format keyword comes from the modal → timestamped default name;
    // anything else is treated as the target filename (TS-style).
    let (format, filename) = match ExportFormat::from_keyword(&arg.to_ascii_lowercase()) {
        Some(format) => {
            let ts = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
            (format, format!("conversation-{ts}.{}", format.ext()))
        }
        None => {
            let format = ExportFormat::from_filename(arg);
            // Append the inferred extension when the filename carries none.
            let filename = if arg.contains('.') {
                arg.to_string()
            } else {
                format!("{arg}.{}", format.ext())
            };
            (format, filename)
        }
    };
    // Render under the lock, then drop it before the file write / await.
    let body = {
        let history = runtime.history().lock().await;
        format.render(history.as_slice())
    };
    let path = runtime.original_cwd().join(&filename);
    let message = match tokio::fs::write(&path, body).await {
        Ok(()) => format!("Conversation exported to {}", path.display()),
        Err(e) => format!("Failed to write export to {}: {e}", path.display()),
    };
    emit_slash_text(event_tx, "export", args, &message).await;
    SlashOutcome::Handled
}

/// `/rename [name]` runner — resolves the new name (explicit or
/// Fast-role auto-generated), persists it via local AppServer
/// `session/rename`, and
/// surfaces a single system-line confirmation.
pub(super) async fn run_session_rename(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
    request: coco_commands::ParsedRename,
) {
    use coco_agent_host::session_rename::auto_generate_session_name;

    // Teammate guard — names are set by the team leader.
    if coco_coordinator::identity::is_teammate() {
        emit_slash_text(
            event_tx,
            "rename",
            "",
            "Cannot rename: This session is a swarm teammate. \
             Teammate names are set by the team leader.",
        )
        .await;
        return;
    }

    // Resolve the new name. `Auto` runs the Fast-role generator
    // against `messages_after_compact_boundary`.
    let name = match request {
        coco_commands::ParsedRename::Explicit(n) => n,
        coco_commands::ParsedRename::Auto => match auto_generate_session_name(session).await {
            Ok(n) => n,
            Err(err) => {
                emit_slash_text(event_tx, "rename", "", err.user_message()).await;
                return;
            }
        },
    };

    let text = match local_app_server_bridge
        .client()
        .session_rename(
            local_app_server_bridge.handler(),
            coco_types::SessionRenameParams { name: name.clone() },
        )
        .await
    {
        Ok(result) => format!("Session renamed to: {}", result.name),
        Err(coco_app_server_client::ClientError::Server { message, .. })
            if message.starts_with("Cannot rename:") =>
        {
            message
        }
        Err(error) => format!("Failed to rename session: {error}"),
    };
    emit_slash_text(event_tx, "rename", "", &text).await;
}

/// `/reload-plugins` runner — rescans plugin + skill dirs and
/// atomically swaps the active `CommandRegistry`. Snapshots taken by
/// in-flight dispatches stay valid (they hold the prior `Arc`); the
/// swap is observed by the next dispatch.
/// After the swap we also push the fresh visible-command list to the
/// TUI via [`TuiOnlyEvent::AvailableCommandsRefreshed`] so the `/`
/// autocomplete popup and command palette stop pointing at stale names
/// from removed plugins.
pub(super) async fn run_reload_plugins(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    let result = match local_app_server_bridge
        .client()
        .plugin_reload(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let body = format!("Plugin reload failed: {error}");
            emit_slash_text(event_tx, "reload-plugins", "", &body).await;
            return;
        }
    };
    let hook_note = if result.error_count == 0 {
        String::new()
    } else {
        format!(" · {} reload error(s)", result.error_count)
    };
    let body = format!(
        "Reloaded — {} commands{hook_note}; agents + LSP refreshed.",
        result.commands.len()
    );
    emit_slash_text(event_tx, "reload-plugins", "", &body).await;

    let snapshot = runtime.current_command_registry().await.snapshot_for_ui();
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
            commands: snapshot,
        }))
        .await;
}

/// `/hooks reload` runner — rebuild the live `HookRegistry` from the
/// latest `RuntimeConfig` snapshot.
pub(super) async fn run_reload_hooks(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let body = match local_app_server_bridge
        .client()
        .hook_reload(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => format!(
            "Reloaded — {} hook(s) registered from current settings.",
            result.hook_count
        ),
        Err(error) => format!("Hook reload failed: {error}"),
    };
    emit_slash_text(event_tx, "hooks", "", &body).await;
}

pub(super) async fn run_show_cost(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_cost(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => emit_slash_text(event_tx, "cost", "", &result.text).await,
        Err(error) => {
            let body = format!("Failed to read session cost: {error}");
            emit_slash_text(event_tx, "cost", "", &body).await;
        }
    }
}

pub(super) async fn run_show_status(
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_status(local_app_server_bridge.handler())
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus {
                    title: "Status".to_string(),
                    body: result.text,
                }))
                .await;
        }
        Err(error) => {
            let body = format!("Failed to read session status: {error}");
            emit_slash_text(event_tx, "status", "", &body).await;
        }
    }
}

/// persisted to settings.json.
pub(super) async fn dispatch_add_dir(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> SlashOutcome {
    let runtime = session;
    let raw_path = args.trim();
    let current_cwd = runtime.current_cwd().read().await.clone();
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        current_cwd.join(raw_path)
    };
    let absolute = match candidate.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            emit_slash_text(
                event_tx,
                "add-dir",
                args,
                &format!("Cannot add directory `{raw_path}`: {e}"),
            )
            .await;
            return SlashOutcome::Handled;
        }
    };
    if !absolute.is_dir() {
        emit_slash_text(
            event_tx,
            "add-dir",
            args,
            &format!(
                "Cannot add directory `{}`: not a directory",
                absolute.display()
            ),
        )
        .await;
        return SlashOutcome::Handled;
    }

    let current = canonicalize_or_self(current_cwd);
    let additional_dirs: Vec<PathBuf> = runtime
        .app_state()
        .read()
        .await
        .permissions
        .additional_dirs
        .values()
        .map(|dir| canonicalize_or_self(PathBuf::from(&dir.path)))
        .collect();

    if let Some(message) = add_dir_already_message(&absolute, &current, &additional_dirs) {
        emit_slash_text(event_tx, "add-dir", args, &message).await;
        return SlashOutcome::Handled;
    }

    let path = absolute.to_string_lossy().into_owned();
    if !apply_session_add_directory(&path, event_tx, local_app_server_bridge).await {
        return SlashOutcome::Handled;
    }
    emit_slash_text(
        event_tx,
        "add-dir",
        args,
        &format!("Added {} as a working directory.", absolute.display()),
    )
    .await;
    SlashOutcome::Handled
}

pub(super) fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

pub(super) fn add_dir_already_message(
    directory_path: &Path,
    current_cwd: &Path,
    additional_dirs: &[PathBuf],
) -> Option<String> {
    if directory_path == current_cwd {
        return Some(format!(
            "{} is already the current working directory.",
            directory_path.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path == working_dir {
            return Some(format!(
                "{} is already added as a working directory.",
                directory_path.display()
            ));
        }
    }
    if directory_path.starts_with(current_cwd) {
        return Some(format!(
            "{} is already accessible within the current working directory {}.",
            directory_path.display(),
            current_cwd.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path.starts_with(working_dir) {
            return Some(format!(
                "{} is already accessible within the additional working directory {}.",
                directory_path.display(),
                working_dir.display()
            ));
        }
    }
    None
}

pub(super) async fn apply_session_add_directory(
    path: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> bool {
    apply_and_persist_permission_update(
        &coco_types::PermissionUpdate::AddDirectories {
            directories: vec![path.to_string()],
            destination: coco_types::PermissionUpdateDestination::Session,
        },
        event_tx,
        local_app_server_bridge,
    )
    .await
}

/// `/tag <name>` runner — toggles the tag via `SessionManager`. Reports
/// "added" or "removed" so the user knows the new state.
pub(super) async fn run_session_tag(
    _session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
    tag: &str,
) {
    let result = local_app_server_bridge
        .client()
        .session_toggle_tag(
            local_app_server_bridge.handler(),
            coco_types::SessionToggleTagParams {
                tag: tag.to_string(),
            },
        )
        .await;
    let text = match result {
        Ok(result) if result.added => format!("Tag added: {}", result.tag),
        Ok(result) => format!("Tag removed: {}", result.tag),
        Err(error) => format!("Failed to toggle tag `{tag}`: {error}"),
    };
    emit_slash_text(event_tx, "tag", tag, &text).await;
}

/// `/permissions allow|deny|reset` dispatch with live-base mutation.
/// The static registry handler can return text but can't mutate the live
/// `ToolAppState.permissions` base. This intercepts the three mutating
/// subcommands so they take real effect — routing allow/deny through
/// `control/applyPermissionUpdate` (live base + disk persist) and reset
/// through local AppServer runtime control; `list` /
/// no-arg fall through to the registry handler that reads settings.json.
/// Returns `None` for non-mutating args so the caller falls through.
/// `/color <name|default>` — set the prompt bar color for this session.
/// Persists to the live `ToolAppState.agent_color` so the prompt-bar UI
/// sees the change without a session restart. Returns `None` for the
/// empty-args case so the registry handler still produces the
/// "Available colors: …" listing.
pub(super) async fn dispatch_color(
    args: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> Option<SlashOutcome> {
    use coco_coordinator::identity::is_teammate;
    use coco_types::AgentColorName;

    if is_teammate() {
        emit_slash_text(
            event_tx,
            "color",
            args,
            "Cannot set color: This session is a swarm teammate. \
             Teammate colors are assigned by the team leader.",
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    let trimmed = args.trim();
    if trimmed.is_empty() {
        // Empty args fall through to the registry handler, which
        // produces the canonical "Please provide a color..." listing
        // (identical to the registry handler's empty-args output).
        return None;
    }

    // Reset aliases.
    const RESET_ALIASES: &[&str] = &["default", "reset", "none", "gray", "grey"];
    let lower = trimmed.to_ascii_lowercase();
    if RESET_ALIASES.contains(&lower.as_str()) {
        if !set_agent_color(None, event_tx, local_app_server_bridge).await {
            return Some(SlashOutcome::Handled);
        }
        emit_slash_text(event_tx, "color", args, "Session color reset to default").await;
        return Some(SlashOutcome::Handled);
    }

    match lower.parse::<AgentColorName>() {
        Ok(color) => {
            if !set_agent_color(Some(color), event_tx, local_app_server_bridge).await {
                return Some(SlashOutcome::Handled);
            }
            emit_slash_text(
                event_tx,
                "color",
                args,
                &format!("Session color set to: {color}"),
            )
            .await;
            Some(SlashOutcome::Handled)
        }
        Err(_) => {
            let list = AgentColorName::ALL
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            emit_slash_text(
                event_tx,
                "color",
                args,
                &format!("Invalid color \"{lower}\". Available colors: {list}, default"),
            )
            .await;
            Some(SlashOutcome::Handled)
        }
    }
}

pub(super) async fn dispatch_permissions_mutation(
    args: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> Option<SlashOutcome> {
    use coco_types::PermissionBehavior;
    use coco_types::PermissionRule;
    use coco_types::PermissionRuleSource;
    use coco_types::PermissionRuleValue;

    // Empty `allow` / `deny` (no tool name) is a usage error — surface
    // the hint without falling through to the registry handler. The
    // pure parser returns `None` in that case (vs. None for read-only
    // / unrecognized which DO fall through).
    let trimmed = args.trim();
    if trimmed == "allow" || trimmed.starts_with("allow  ") || trimmed == "allow " {
        // Route through the typed status enum so the TUI translates via
        // `slash.permissions.usage_allow` (i18n parity with the other
        // dispatcher status messages).
        emit_slash_status(
            event_tx,
            "permissions",
            args,
            SlashCommandStatusKind::PermissionsUsageAllow,
        )
        .await;
        return Some(SlashOutcome::Handled);
    }
    if trimmed == "deny" || trimmed.starts_with("deny  ") || trimmed == "deny " {
        emit_slash_status(
            event_tx,
            "permissions",
            args,
            SlashCommandStatusKind::PermissionsUsageDeny,
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    let mutation = parse_permissions_mutation(args)?;

    let confirmation = match &mutation {
        PermissionsMutation::Allow(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Allow,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            if !apply_and_persist_permission_update(
                &coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            format!(
                "Added allow rule for `{tool}`.\n\nSource: Session (highest priority — \
                 active until end of session or `/permissions reset`)."
            )
        }
        PermissionsMutation::Deny(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Deny,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            if !apply_and_persist_permission_update(
                &coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            format!(
                "Added deny rule for `{tool}`.\n\nSource: Session (highest priority — \
                 active until end of session or `/permissions reset`)."
            )
        }
        PermissionsMutation::Reset => {
            // Reset is Session-source-only and never persists to disk. Route
            // through AppServer so the TUI does not mutate SessionRuntime.
            if !reset_session_permission_rules(event_tx, local_app_server_bridge).await {
                return Some(SlashOutcome::Handled);
            }
            {
                let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
                format!(
                    "Session permission rules reset. Custom session allow/deny entries were cleared; \
                     built-in read-only tools remain allowed by the active permission mode. File-based rules \
                     ({config_dir}/settings.json, ~/{config_dir}/settings.json) are unchanged — \
                     edit those files directly to modify persistent rules."
                )
            }
        }
    };
    emit_slash_text(event_tx, "permissions", args, &confirmation).await;
    Some(SlashOutcome::Handled)
}
