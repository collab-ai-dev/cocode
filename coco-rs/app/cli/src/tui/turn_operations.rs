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
        coco_agent_host::app_session::AppSessionHandle,
    >,
    pub(super) handler: coco_agent_host::app_server_host::AppServerHostHandler,
    pub(super) target: coco_types::SessionTarget,
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
    /// `/journey` `e` on the selected node → fork `$EDITOR` against the node's
    /// backing file (a SKILL.md or a memory entry). Refreshes the journey
    /// snapshot on editor exit, mirroring [`Self::Agent`].
    Journey {
        path: std::path::PathBuf,
    },
}

/// Upper bound for `ActiveTurnDrain::Wait`: long enough for any legitimate
/// post-interrupt drain, short enough that a monitor stuck on a lag-dropped
/// `TurnEnded` cannot freeze the driver loop indefinitely.
const WAIT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
        let ActiveTurnCancel {
            client,
            handler,
            target,
        } = &s.cancel;
        if let Err(error) = client.turn_interrupt(handler, target.clone()).await {
            tracing::warn!(%error, "drain_active_turn: AppServer turn/interrupt failed");
        }
        match mode {
            ActiveTurnDrain::Wait => {
                // Bounded: the monitor's TurnEnded can be lost to broadcast
                // lag, and an unbounded wait here freezes the whole driver
                // loop (next SubmitInput / /clear / /compact). On timeout the
                // stale monitor is aborted; if the turn is somehow still
                // running the server rejects the next start with
                // TurnAlreadyRunning, which surfaces as a visible error
                // instead of a frozen UI.
                let mut task = s.task;
                tokio::select! {
                    result = &mut task => {
                        let _ = result;
                    }
                    _ = tokio::time::sleep(WAIT_DRAIN_TIMEOUT) => {
                        warn!(
                            timeout_ms = WAIT_DRAIN_TIMEOUT.as_millis(),
                            "active turn monitor did not observe TurnEnded; aborting stale monitor"
                        );
                        task.abort();
                        let _ = task.await;
                    }
                }
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    custom_instructions: Option<String>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
) {
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
    if local_app_server_bridge.is_child_session(session.session_id()) {
        let Some(client) = full_session_for(local_app_server_bridge, session.session_id()) else {
            warn!(session_id = %session.session_id(), "TUI sidechat /compact session is stale");
            return;
        };
        let params = coco_types::TurnStartParams {
            target: client.session_target(),
            prompt,
            images: Vec::new(),
            composer: Default::default(),
            slash_metadata: None,
            model_selection: None,
            permission_mode: None,
            thinking_level: None,
            goal_continuation: false,
        };
        if let Err(error) = local_app_server_bridge.start_child_turn(params).await {
            warn!(%error, "TUI sidechat /compact failed");
        }
        return;
    }

    // Primary compaction mutates primary history and therefore drains only the
    // primary foreground turn. Sidechat compaction above owns a separate turn
    // coordinator and can run concurrently.
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    prompt: String,
    log_label: &'static str,
) {
    let session_id = session.session_id().clone();
    if let Err(error) = local_app_server_bridge
        .activate_existing_full_session(session_id.clone(), Some(event_tx.clone()))
    {
        tracing::warn!(%error, "{log_label} could not activate local AppServer session");
        return;
    }
    let mut monitor_client = local_app_server_bridge.connect_local_client();
    // live-only tail attach with no replay cursor (see SubmitInput site).
    let observed_session = monitor_client.observe_session(session_id.clone());
    let params = coco_types::TurnStartParams {
        target: session_target(local_app_server_bridge),
        prompt,
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
        while let Some(envelope) = monitor_client.next_session_event(&observed_session).await {
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
            target: session_target(local_app_server_bridge),
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

    coco_agent_host::session_slash::invoke_fork_skill_and_append_result(
        runtime,
        event_tx.clone(),
        name,
        args,
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    let runtime = session;
    drain_active_turn(active_turn, ActiveTurnDrain::Wait).await;
    let old_session_id = runtime.session_id().clone();
    if let Err(error) =
        local_app_server_bridge.activate_existing_full_session(old_session_id.clone(), None)
    {
        warn!(
            %error,
            session_id = %old_session_id,
            "/clear could not confirm local AppServer Full grant"
        );
        return;
    }
    let binding = match local_app_server_bridge
        .replace_session_with_clear(Some(event_tx.clone()))
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            warn!(%error, "/clear failed to replace local AppServer session");
            return;
        }
    };
    let new_session = binding.session;
    let new_session_id = new_session.session_id().clone();
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
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

/// `/btw [question]` runner — creates an ephemeral, read-only sidechat child
/// session and optionally starts its first turn.
///
/// The child is a distinct session with its own turn coordinator, so it runs
/// concurrently with the parent and never touches the parent transcript. Its
/// turn events stream into the view through the child event pump. At most one
/// child exists per parent (I-2); a second `/btw` is rejected until the current
/// child is closed from the TUI (Ctrl+C while idle).
pub(super) async fn run_side_chat(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    _active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    _turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    request: coco_commands::handlers::btw::BtwRequest,
    images: &[coco_types::QueuedCommandEditImage],
) {
    let args = match &request {
        coco_commands::handlers::btw::BtwRequest::Open => "",
        coco_commands::handlers::btw::BtwRequest::OpenAndAsk { question } => question,
    };
    let parent_id = session.session_id().clone();

    if local_app_server_bridge.child_full_session().is_some() {
        emit_slash_text(
            event_tx,
            &parent_id,
            "btw",
            args,
            "A sidechat is already open. Continue it with plain input, or press Ctrl+C to return to main.",
        )
        .await;
        return;
    }

    let binding = match local_app_server_bridge
        .open_side_chat(parent_id.clone(), Some(event_tx.clone()))
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            emit_slash_text(
                event_tx,
                &parent_id,
                "btw",
                args,
                &format!("Couldn't start the sidechat: {error}"),
            )
            .await;
            return;
        }
    };
    let child_id = binding.session.session_id().clone();
    let _ = event_tx
        .send(CoreEvent::Tui(coco_types::TuiOnlyEvent::SideChatEntered {
            parent_id: parent_id.clone(),
            child_id: child_id.clone(),
        }))
        .await;
    let coco_commands::handlers::btw::BtwRequest::OpenAndAsk { question } = request else {
        return;
    };
    let params = coco_types::TurnStartParams {
        target: binding.client.session_target(),
        prompt: question.clone(),
        images: images.to_vec(),
        composer: Default::default(),
        slash_metadata: None,
        model_selection: None,
        permission_mode: None,
        thinking_level: None,
        goal_continuation: false,
    };
    if let Err(error) = local_app_server_bridge.start_child_turn(params).await {
        let _ = local_app_server_bridge.close_child().await;
        emit_slash_text(
            event_tx,
            &parent_id,
            "btw",
            &question,
            &format!("Couldn't start the sidechat turn: {error}"),
        )
        .await;
    }
}

/// `/export <filename>` runner — renders the live conversation `MessageHistory`
/// (incl. tool activity) and writes it to a file in the session's original cwd,
/// then confirms the path. Clipboard export lives in `/copy`.
pub(super) async fn run_export(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) -> SlashOutcome {
    let message =
        coco_agent_host::conversation_export::export_conversation_for_session(session, args).await;
    emit_slash_text(event_tx, session.session_id(), "export", args, &message).await;
    SlashOutcome::Handled
}

/// `/rename [name]` runner — resolves the new name (explicit or
/// Fast-role auto-generated), persists it via local AppServer
/// `session/rename`, and
/// surfaces a single system-line confirmation.
pub(super) async fn run_session_rename(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
    request: coco_commands::ParsedRename,
) {
    // Teammate guard — names are set by the team leader.
    if coco_coordinator::identity::is_teammate() {
        emit_slash_text(
            event_tx,
            session.session_id(),
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
    let name =
        match coco_agent_host::session_labels::resolve_rename_name(Some(session), request).await {
            Ok(name) => name,
            Err(error) => {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    "rename",
                    "",
                    &error.user_message(),
                )
                .await;
                return;
            }
        };

    let text = match local_app_server_bridge
        .client()
        .session_rename(
            local_app_server_bridge.handler(),
            coco_types::SessionRenameParams {
                target: coco_types::SessionTarget {
                    session_id: session.session_id().clone(),
                },
                name: name.clone(),
            },
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
    emit_slash_text(event_tx, session.session_id(), "rename", "", &text).await;
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
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    let Some(full_session) = full_session_for(local_app_server_bridge, session.session_id()) else {
        emit_slash_text(
            event_tx,
            session.session_id(),
            "reload-plugins",
            "",
            "Plugin reload failed: this session is no longer available.",
        )
        .await;
        return;
    };
    let result = match local_app_server_bridge
        .client()
        .plugin_reload(local_app_server_bridge.handler(), full_session)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let body = format!("Plugin reload failed: {error}");
            emit_slash_text(event_tx, session.session_id(), "reload-plugins", "", &body).await;
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
    emit_slash_text(event_tx, session.session_id(), "reload-plugins", "", &body).await;

    let snapshot =
        coco_agent_host::session_dialogs::build_available_commands_payload(session).await;
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
            commands: snapshot,
        }))
        .await;
}

/// `/hooks reload` runner — rebuild the live `HookRegistry` from the
/// latest `RuntimeConfig` snapshot.
pub(super) async fn run_reload_hooks(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    let Some(full_session) = full_session_for(local_app_server_bridge, session.session_id()) else {
        emit_slash_text(
            event_tx,
            session.session_id(),
            "hooks",
            "",
            "Hook reload failed: this session is no longer available.",
        )
        .await;
        return;
    };
    let body = match local_app_server_bridge
        .client()
        .hook_reload(local_app_server_bridge.handler(), full_session)
        .await
    {
        Ok(result) => format!(
            "Reloaded — {} hook(s) registered from current settings.",
            result.hook_count
        ),
        Err(error) => format!("Hook reload failed: {error}"),
    };
    emit_slash_text(event_tx, session.session_id(), "hooks", "", &body).await;
}

pub(super) async fn run_show_cost(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_cost(
            local_app_server_bridge.handler(),
            coco_types::SessionTarget {
                session_id: session.session_id().clone(),
            },
        )
        .await
    {
        Ok(result) => {
            emit_slash_text(event_tx, session.session_id(), "cost", "", &result.text).await
        }
        Err(error) => {
            let body = format!("Failed to read session cost: {error}");
            emit_slash_text(event_tx, session.session_id(), "cost", "", &body).await;
        }
    }
}

pub(super) async fn run_show_status(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    match local_app_server_bridge
        .client()
        .session_status(
            local_app_server_bridge.handler(),
            coco_types::SessionTarget {
                session_id: session.session_id().clone(),
            },
        )
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenGoalStatus {
                    session_id: session.session_id().clone(),
                    title: "Status".to_string(),
                    body: result.text,
                }))
                .await;
        }
        Err(error) => {
            let body = format!("Failed to read session status: {error}");
            emit_slash_text(event_tx, session.session_id(), "status", "", &body).await;
        }
    }
}

/// Adds a session-scoped working directory through AppServer permissions.
pub(super) async fn dispatch_add_dir(
    args: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> SlashOutcome {
    let prepared =
        match coco_agent_host::session_controls::prepare_directory_access_update(session, args)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                emit_slash_text(
                    event_tx,
                    session.session_id(),
                    "add-dir",
                    args,
                    &error.to_string(),
                )
                .await;
                return SlashOutcome::Handled;
            }
        };
    let absolute = prepared.path;
    let path = absolute.to_string_lossy().into_owned();
    if !apply_session_add_directory(
        &path,
        session.session_id(),
        event_tx,
        local_app_server_bridge,
    )
    .await
    {
        return SlashOutcome::Handled;
    }
    emit_slash_text(
        event_tx,
        session.session_id(),
        "add-dir",
        args,
        &format!("Added {} as a working directory.", absolute.display()),
    )
    .await;
    SlashOutcome::Handled
}

pub(super) async fn apply_session_add_directory(
    path: &str,
    session_id: &coco_types::SessionId,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> bool {
    apply_and_persist_permission_update(
        &coco_types::PermissionUpdate::AddDirectories {
            directories: vec![path.to_string()],
            destination: coco_types::PermissionUpdateDestination::Session,
        },
        session_id,
        event_tx,
        local_app_server_bridge,
    )
    .await
}

/// `/tag <name>` runner — toggles the tag via `SessionManager`. Reports
/// "added" or "removed" so the user knows the new state.
pub(super) async fn run_session_tag(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
    tag: &str,
) {
    let result = local_app_server_bridge
        .client()
        .session_toggle_tag(
            local_app_server_bridge.handler(),
            coco_types::SessionToggleTagParams {
                target: coco_types::SessionTarget {
                    session_id: session.session_id().clone(),
                },
                tag: tag.to_string(),
            },
        )
        .await;
    let text = match result {
        Ok(result) if result.added => format!("Tag added: {}", result.tag),
        Ok(result) => format!("Tag removed: {}", result.tag),
        Err(error) => format!("Failed to toggle tag `{tag}`: {error}"),
    };
    emit_slash_text(event_tx, session.session_id(), "tag", tag, &text).await;
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
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> Option<SlashOutcome> {
    use coco_coordinator::identity::is_teammate;
    use coco_types::AgentColorName;

    if is_teammate() {
        emit_slash_text(
            event_tx,
            session.session_id(),
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
        if !set_agent_color(
            None,
            session.session_id(),
            event_tx,
            local_app_server_bridge,
        )
        .await
        {
            return Some(SlashOutcome::Handled);
        }
        emit_slash_text(
            event_tx,
            session.session_id(),
            "color",
            args,
            "Session color reset to default",
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    match lower.parse::<AgentColorName>() {
        Ok(color) => {
            if !set_agent_color(
                Some(color),
                session.session_id(),
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            emit_slash_text(
                event_tx,
                session.session_id(),
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
                session.session_id(),
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
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) -> Option<SlashOutcome> {
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
            session.session_id(),
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
            session.session_id(),
            "permissions",
            args,
            SlashCommandStatusKind::PermissionsUsageDeny,
        )
        .await;
        return Some(SlashOutcome::Handled);
    }

    let action = coco_agent_host::session_controls::permission_mutation_action(args)?;

    let confirmation = match action {
        coco_agent_host::session_controls::PermissionMutationAction::Apply {
            update,
            confirmation,
        } => {
            if !apply_and_persist_permission_update(
                &update,
                session.session_id(),
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            confirmation
        }
        coco_agent_host::session_controls::PermissionMutationAction::Reset { confirmation } => {
            // Reset is Session-source-only and never persists to disk. Route
            // through AppServer so the TUI does not mutate SessionRuntime.
            if !reset_session_permission_rules(
                session.session_id(),
                event_tx,
                local_app_server_bridge,
            )
            .await
            {
                return Some(SlashOutcome::Handled);
            }
            confirmation
        }
    };
    emit_slash_text(
        event_tx,
        session.session_id(),
        "permissions",
        args,
        &confirmation,
    )
    .await;
    Some(SlashOutcome::Handled)
}
