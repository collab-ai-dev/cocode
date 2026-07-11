use super::*;

/// Emit a `TuiOnlyEvent::SlashCommandResult` so the TUI appends a
/// system-role chat message carrying handler-rendered content (verbatim,
/// no translation).
pub(super) async fn emit_slash_text(
    event_tx: &mpsc::Sender<CoreEvent>,
    name: &str,
    args: &str,
    text: &str,
) {
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandResult {
            name: name.to_string(),
            args: args.to_string(),
            text: text.to_string(),
        }))
        .await;
}

pub(super) async fn dispatch_context(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) -> SlashOutcome {
    local_app_server_bridge
        .install_session_runtime(session.clone())
        .await;
    match local_app_server_bridge
        .client()
        .context_usage(
            local_app_server_bridge.handler(),
            session_target(local_app_server_bridge),
        )
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenContextUsage { result }))
                .await;
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                "context",
                /*args*/ "",
                SlashCommandStatusKind::Failed {
                    error: e.to_string(),
                },
            )
            .await;
        }
    }
    SlashOutcome::Handled
}

/// Optional `/login <provider>` arg → instance name. Empty → builtin default.
pub(super) fn slash_provider_arg(args: &str) -> Option<String> {
    let a = args.trim();
    (!a.is_empty()).then(|| a.to_string())
}

/// `/login [provider]` — runs the OAuth flow on the shared `AuthService`, shows
/// the authorize URL + result in the transcript. Loopback-only (the TUI owns
/// stdin, so the paste fallback isn't available in-session — use `coco login
/// --no-browser` on a plain terminal for that).
/// Rebuild provider availability and push it to the TUI so the `/model`
/// picker reflects a credential change (login/logout) without a restart.
/// Only `provider_statuses` is auth-dependent — the model catalog and role
/// map derive from static config and are left untouched.
/// plan empty) fails.
pub(super) async fn maybe_spawn_auto_title(
    session: &crate::session_runtime::SessionHandle,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    session_id: &coco_types::SessionId,
    client: coco_agent_host::local_client::LocalServerClient<
        coco_agent_host::sdk_server::LocalAppSessionHandle,
    >,
    handler: coco_agent_host::sdk_server::AppServerSdkHandler,
) {
    let runtime = session;
    let plan_exited = runtime.app_state().read().await.has_exited_plan_mode;
    let plans_dir = coco_context::resolve_plans_directory(
        runtime.config_home(),
        /*project_dir*/ None,
        /*setting*/ None,
    );
    let plan_text = coco_context::get_plan(session_id.as_str(), &plans_dir, /*agent_id*/ None);
    let plan_non_empty = plan_text
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    let already_attempted = title_gen_attempted
        .read()
        .await
        .contains(session_id.as_str());
    if !should_trigger_title_gen(
        runtime.auto_title_enabled(),
        already_attempted,
        runtime.fast_model_spec().is_some(),
        plan_exited,
        plan_non_empty,
    ) {
        return;
    }
    let (Some(_spec), Some(text)) = (runtime.fast_model_spec().cloned(), plan_text) else {
        return;
    };
    title_gen_attempted
        .write()
        .await
        .insert(session_id.to_string());
    spawn_auto_title_task(session.clone(), text, client, handler);
}

/// Synchronous TUI-cancel cleanup.
/// Truncates the runtime history at the target user message and emits
/// the authoritative `MessageTruncated` event so SDK + TUI observers
/// converge. Never touches the workspace — file rewind belongs to the
/// explicit [`handle_rewind`] flow. See
/// `engine-tui-unified-transcript-plan.md` §7.4.
pub(super) async fn handle_auto_truncate(
    message_id: &str,
    event_tx: &mpsc::Sender<CoreEvent>,
    session: &crate::session_runtime::SessionHandle,
) {
    let runtime = session;
    let mut h = runtime.history().lock().await;
    let Some(idx) = h.as_slice().iter().position(|m| match m.as_ref() {
        coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
        _ => false,
    }) else {
        // Auto-restore is fire-and-forget; if the target uuid is gone
        // (e.g. a compaction wiped it between TUI dispatch and engine
        // handler), we'd rather skip silently than panic. `warn` so
        // ops can correlate "auto-restore quietly did nothing" with
        // an upstream truncation race.
        tracing::warn!(
            target: "coco_agent_host::auto_truncate",
            message_id,
            history_len = h.len(),
            "AutoTruncate target message not found in history (likely raced with compaction)",
        );
        return;
    };
    let pre_count = h.len() as i32;
    let removed = (pre_count - idx as i32).max(0);
    h.truncate(idx);
    tracing::info!(
        target: "coco_agent_host::auto_truncate",
        message_id,
        keep_count = idx,
        removed,
        "AutoTruncate applied",
    );
    coco_otel::events::emit_conversation_rewind(
        pre_count as i64,
        h.len() as i64,
        removed as i64,
        idx as i64,
    );
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::MessageTruncated {
            keep_count: idx as i64,
            identity: coco_types::ServerNotificationIdentity::default(),
        }))
        .await;
}

/// Explicit `/rewind` command driver — picker-confirmed.
/// Branches on `restore_type`:
/// - `Both` / `CodeOnly` — `file_history.rewind()` restores files.
/// - `Both` / `ConversationOnly` — truncate history and emit
/// `MessageTruncated`.
/// - `SummarizeFrom` / `SummarizeUpTo` — dispatch to
/// `handle_summarize_rewind` (partial compaction).
/// Always emits `RewindCompleted` so the TUI dismisses the picker overlay.
pub(super) async fn handle_rewind(
    restore_type: &coco_tui::state::RestoreType,
    message_id: &str,
    rewound_turn: i32,
    event_tx: &mpsc::Sender<CoreEvent>,
    session: &crate::session_runtime::SessionHandle,
    local_app_server_bridge: &mut coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    use coco_tui::state::RestoreType;

    let mut files_changed = 0i32;
    let mut messages_removed = 0i32;
    let mut keep_count_to_emit = None;
    let mut history_replacement_to_emit: Option<Vec<coco_messages::Message>> = None;

    tracing::info!(
        target: "coco_agent_host::rewind",
        message_id,
        rewound_turn,
        ?restore_type,
        "Explicit rewind: dispatching",
    );

    // Summarize variants: dispatch to partial_compact_conversation
    // and replace the history with the resulting messages.
    if matches!(
        restore_type,
        RestoreType::SummarizeFrom { .. } | RestoreType::SummarizeUpTo { .. }
    ) {
        handle_summarize_rewind(restore_type, message_id, session, event_tx).await;
        return;
    }

    // Code rewind (file restore)
    // CodeOnly + Both restore files; Summarize variants do NOT
    // restore files — summarize keeps the workspace intact, only
    // the conversation is rewritten.
    if matches!(restore_type, RestoreType::Both | RestoreType::CodeOnly)
        && runtime.file_history().is_some()
    {
        local_app_server_bridge
            .install_session_runtime(session.clone())
            .await;
        let session_id = runtime.current_typed_session_id().await;
        if let Err(error) = local_app_server_bridge.ensure_interactive_surface(session_id) {
            tracing::warn!(%error, "rewind could not attach interactive AppServer surface");
            return;
        }
        match local_app_server_bridge
            .client()
            .rewind_files(
                local_app_server_bridge.handler(),
                coco_types::RewindFilesParams {
                    target: interactive_target(local_app_server_bridge),
                    user_message_id: message_id.to_string(),
                    dry_run: false,
                },
            )
            .await
        {
            Ok(result) => {
                files_changed = result.files_changed.len() as i32;
                info!(files_changed, message_id, "File history rewind completed");
            }
            Err(error) => {
                warn!("File history rewind failed: {error}");
                let _ = event_tx
                    .send(CoreEvent::Protocol(ServerNotification::Error(
                        coco_types::ErrorParams {
                            message: format!("File rewind failed: {error}"),
                            category: Some("rewind".into()),
                            retryable: false,
                        },
                    )))
                    .await;
                return;
            }
        }
    }

    // Conversation rewind: truncate the agent-side history at the
    // target message, emit TuiOnlyEvent so the TUI mirrors the
    // truncate on its display side.
    let should_truncate = matches!(
        restore_type,
        RestoreType::Both | RestoreType::ConversationOnly
    );

    if should_truncate {
        let mut h = runtime.history().lock().await;
        match h.as_slice().iter().position(|m| match m.as_ref() {
            coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
            _ => false,
        }) {
            Some(idx) => {
                let pre_count = h.len() as i32;
                messages_removed = (pre_count - idx as i32).max(0);
                h.truncate(idx);
                tracing::info!(
                    target: "coco_agent_host::rewind",
                    message_id,
                    keep_count = idx,
                    messages_removed,
                    files_changed,
                    "Explicit rewind: truncated history",
                );
                coco_otel::events::emit_conversation_rewind(
                    pre_count as i64,
                    h.len() as i64,
                    messages_removed as i64,
                    idx as i64,
                );
                keep_count_to_emit = Some(idx as i64);
            }
            None => {
                let history_len = h.len();
                drop(h);
                if let Some((keep_count, removed, kept)) =
                    runtime.restore_pre_clear_rewind_prefix(message_id).await
                {
                    messages_removed = removed;
                    keep_count_to_emit = Some(keep_count as i64);
                    history_replacement_to_emit = Some(kept);
                    tracing::info!(
                        target: "coco_agent_host::rewind",
                        message_id,
                        keep_count,
                        messages_removed,
                        "Explicit rewind: restored pre-clear history prefix",
                    );
                } else {
                    tracing::warn!(
                        target: "coco_agent_host::rewind",
                        message_id,
                        history_len,
                        "Explicit rewind: target user message not found in history",
                    );
                }
            }
        }
    }

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::RewindCompleted {
            target_message_id: if should_truncate {
                message_id.to_string()
            } else {
                String::new()
            },
            files_changed,
        }))
        .await;

    // Explicit-rewind converges on the same `MessageTruncated` event the
    // AutoRestore path emits, but it must arrive after the TUI-only
    // completion event. `on_rewind_completed` restores the selected prompt
    // from the still-intact transcript before this truncation applies.
    if let Some(keep_count) = keep_count_to_emit {
        if let Some(messages) = history_replacement_to_emit {
            let _ = event_tx
                .send(CoreEvent::Protocol(ServerNotification::HistoryReplaced {
                    messages: messages.into_iter().map(Arc::new).collect(),
                    identity: coco_types::ServerNotificationIdentity::default(),
                    reason: coco_types::HistoryReplaceReason::Rewind,
                }))
                .await;
        }
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::MessageTruncated {
                keep_count,
                identity: coco_types::ServerNotificationIdentity::default(),
            }))
            .await;
    }

    // Protocol-level event for SDK consumers (Phase 3.2).
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::RewindCompleted(
            coco_types::RewindCompletedParams {
                rewound_turn,
                restored_files: files_changed,
                messages_removed,
            },
        )))
        .await;
}

/// Run `partial_compact_conversation` for SummarizeFrom / SummarizeUpTo
/// rewind options, replace the agent history with the result, and
/// emit a TUI signal to mirror the truncation in the display.
/// Direction mapping: `SummarizeFrom` == `Newest`; `SummarizeUpTo` == `Oldest`.
pub(super) async fn handle_summarize_rewind(
    restore_type: &coco_tui::state::RestoreType,
    message_id: &str,
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let runtime = session;
    use coco_messages::PartialCompactDirection;
    use coco_tui::state::RestoreType;

    let (direction, feedback) = match restore_type {
        RestoreType::SummarizeFrom { feedback } => (PartialCompactDirection::Newest, feedback),
        RestoreType::SummarizeUpTo { feedback } => (PartialCompactDirection::Oldest, feedback),
        _ => return,
    };

    let messages: Vec<std::sync::Arc<coco_messages::Message>> = {
        let h = runtime.history().lock().await;
        h.as_slice().to_vec()
    };

    // Pivot index: position of the picked user message in the
    // history vec.
    let pivot_index = match messages.iter().position(|m| match m.as_ref() {
        coco_messages::Message::User(u) => u.uuid.to_string() == message_id,
        _ => false,
    }) {
        Some(i) => i,
        None => {
            warn!(
                message_id,
                "summarize-rewind: target message not found in history"
            );
            let _ = event_tx
                .send(CoreEvent::Protocol(coco_query::ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: "summarize: message not in active history".into(),
                        category: Some("rewind".into()),
                        retryable: false,
                    },
                )))
                .await;
            return;
        }
    };

    let engine = runtime.build_engine(CancellationToken::new()).await;
    let mut history = coco_messages::MessageHistory::new();
    for arc in messages {
        history.push_arc(arc);
    }
    let event_tx_opt = Some(event_tx.clone());
    let outcome = engine
        .run_partial_compact(
            &mut history,
            &event_tx_opt,
            pivot_index,
            direction,
            feedback.clone(),
            /*custom_instructions*/ None,
        )
        .await;

    match outcome {
        coco_compact::CompactOutcome::Applied => {
            {
                let mut h = runtime.history().lock().await;
                *h = history;
            }
            // Emit a RewindCompleted with empty target so the TUI
            // dismisses the modal + shows a toast, but does NOT try
            // to truncate by message_id (the message is gone after
            // summarization).
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::RewindCompleted {
                    target_message_id: String::new(),
                    files_changed: 0,
                }))
                .await;
        }
        coco_compact::CompactOutcome::Skipped | coco_compact::CompactOutcome::Failed => {
            warn!("partial-compact rewind failed");
            let _ = event_tx
                .send(CoreEvent::Protocol(coco_query::ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: "Summarize failed".into(),
                        category: Some("rewind".into()),
                        retryable: false,
                    },
                )))
                .await;
        }
    }
}

/// Decide whether the driver should fire an auto-title task this turn.
/// Pure gate function factored out of the driver loop so we can unit
/// test the precedence without spinning up a real engine. All five
/// conditions must hold; missing any single one short-circuits.
pub(super) fn should_trigger_title_gen(
    auto_title_enabled: bool,
    already_attempted: bool,
    fast_spec_present: bool,
    plan_has_exited: bool,
    plan_text_non_empty: bool,
) -> bool {
    auto_title_enabled
        && !already_attempted
        && fast_spec_present
        && plan_has_exited
        && plan_text_non_empty
}

/// Spawn a detached tokio task that auto-names the session from the approved
/// plan text via the same generator used by bare `/rename`.
pub(super) fn spawn_auto_title_task(
    session: crate::session_runtime::SessionHandle,
    plan_text: String,
    client: coco_agent_host::local_client::LocalServerClient<
        coco_agent_host::sdk_server::LocalAppSessionHandle,
    >,
    handler: coco_agent_host::sdk_server::AppServerSdkHandler,
) {
    tokio::spawn(async move {
        let session_id = session.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if session
            .session_manager()
            .load(&session_id_string)
            .map(|session| session.title.is_some())
            .unwrap_or(false)
        {
            return;
        }

        let plan_head = plan_text.chars().take(1_000).collect::<String>();
        let Ok(name) = coco_agent_host::session_rename::generate_session_name_from_text(
            session.side_query(),
            plan_head,
        )
        .await
        else {
            return;
        };
        let session_id = session.current_typed_session_id().await;
        let session_id_string = session_id.to_string();
        if session
            .session_manager()
            .load(&session_id_string)
            .map(|session| session.title.is_some())
            .unwrap_or(false)
        {
            return;
        }
        let _ = client
            .session_rename(
                &handler,
                coco_types::SessionRenameParams {
                    target: coco_types::SessionTarget {
                        session_id: session_id.clone(),
                    },
                    name,
                },
            )
            .await;
    });
}

/// Persist a `skill_overrides` JSON patch to
/// `project config dir/settings.local.json`, refresh the in-process
/// registry, and notify the TUI so the dialog's toast + `/`
/// autocomplete pick up the change.
/// **No user-visible string generation here** — the localized
/// "Updated N / No changes / Failed: …" toast is rendered by the
/// TUI from the `SkillOverridesSaved` event payload (the i18n
/// catalog is anchored at `coco-tui` and can't be reached from
/// `coco-cli`).
/// Steps:
/// - Atomic write to `project config dir/settings.local.json` via
/// [`coco_config::LocalSettingsWriter::write_local`] — the writer
/// also republishes `RuntimeConfig` synchronously so the next
/// agent turn reads the new tiers.
/// - Rebuild the command registry against the freshly-published
/// `RuntimeConfig` (NOT the stale snapshot in
/// `runtime.runtime_config()`) so the `off`-overridden skills drop
/// out of the visible command set.
/// - Push `AvailableCommandsRefreshed` so the TUI's `/`
/// autocomplete updates in the same frame.
/// - Emit `SkillOverridesSaved` so the TUI renders the localized
/// toast.
pub(super) async fn handle_write_skill_overrides(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    patch: serde_json::Value,
    runtime_publisher: Option<&Arc<coco_config::RuntimePublisher>>,
    cwd: &std::path::Path,
    flag_settings: Option<&std::path::Path>,
) {
    let runtime = session;
    let result = match runtime_publisher {
        Some(publisher) => {
            let catalogs = coco_config::CatalogPaths::default();
            let roots =
                coco_config::SettingsRoots::new(runtime.project_root().clone(), cwd.to_path_buf());
            let write_result = coco_config::write_local_settings_with_roots(
                roots,
                flag_settings.map(std::path::Path::to_path_buf),
                catalogs,
                Arc::clone(publisher),
                patch,
            )
            .await;
            match write_result {
                Ok(()) => {
                    // Use the freshly-republished RuntimeConfig so
                    // the rebuilt registry sees the new tiers — the
                    // snapshot bound to SessionRuntime at startup
                    // would silently drop the changes.
                    let fresh = publisher.current();
                    // Sync the per-session engine_config too. Per-
                    // turn QueryEngine builds clone from
                    // `engine_config.skill_overrides`; without
                    // this update, every PR2 runtime gate
                    // (SkillTool / listing budget / reminder source)
                    // keeps reading the stale snapshot and the
                    // override silently fails to take effect.
                    let fresh_tiers = Arc::new(fresh.skill_overrides.clone());
                    runtime
                        .update_engine_config(move |cfg| {
                            cfg.skill_overrides = fresh_tiers;
                        })
                        .await;
                    let _ = runtime.reload_plugins_with(cwd, &fresh).await;
                    let snapshot = runtime.current_command_registry().await.snapshot_for_ui();
                    let _ = event_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
                            commands: snapshot,
                        }))
                        .await;
                    coco_types::SkillOverridesSaveResult::Ok
                }
                Err(e) => coco_types::SkillOverridesSaveResult::Err {
                    kind: save_error_kind(&e),
                    message: e.to_string(),
                },
            }
        }
        None => coco_types::SkillOverridesSaveResult::Err {
            kind: coco_types::SkillOverridesSaveErrorKind::NoPublisher,
            message: "settings hot-reload disabled; restart the process to pick up changes"
                .to_string(),
        },
    };

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SkillOverridesSaved { result }))
        .await;
}

/// Map a [`coco_config::SettingsWriteError`] to its wire-categorical
/// kind for the TUI to dispatch by category (toast severity / future
/// retry affordance) rather than rely on string parsing.
pub(super) fn save_error_kind(
    e: &coco_config::SettingsWriteError,
) -> coco_types::SkillOverridesSaveErrorKind {
    use coco_config::SettingsWriteError as E;
    use coco_types::SkillOverridesSaveErrorKind as K;
    match e {
        E::Io { .. } => K::Io,
        E::Parse { .. } => K::Parse,
        E::Rebuild { .. } => K::Rebuild,
    }
}

/// Encode TUI paste-pill image bytes as base64 [`QueuedImage`]s for
/// `CommandQueue` storage. `QueuedImage` carries a base64 payload (the
/// shape coco-rs uses for system-reminder image attachments) so we
/// encode once at the bridge and the engine ships it through unchanged.
/// MIME defaults to `image/png` when missing.
pub(super) fn image_data_to_queued(images: &[coco_tui::ImageData]) -> Vec<QueuedImage> {
    use base64::Engine;
    images
        .iter()
        .map(|img| QueuedImage {
            media_type: if img.mime.is_empty() {
                "image/png".to_string()
            } else {
                img.mime.clone()
            },
            data_base64: base64::engine::general_purpose::STANDARD.encode(&img.bytes),
        })
        .collect()
}

pub(super) fn image_data_to_turn_start(
    images: &[coco_tui::ImageData],
) -> Vec<coco_types::QueuedCommandEditImage> {
    image_data_to_queued(images)
        .into_iter()
        .map(|image| coco_types::QueuedCommandEditImage {
            media_type: image.media_type,
            data_base64: image.data_base64,
        })
        .collect()
}

pub(super) fn model_runtime_source_to_turn_start_selection(
    source: Option<coco_inference::ModelRuntimeSource>,
) -> Option<coco_types::ProviderModelSelection> {
    match source {
        Some(coco_inference::ModelRuntimeSource::Explicit(selection)) => Some(selection),
        Some(coco_inference::ModelRuntimeSource::Role(_)) | None => None,
    }
}

/// Run a prompt-mode bash submission (`!ls -la`). The command runs once in the
/// session cwd via [`coco_shell::ShellExecutor`] and the merged stdout+stderr
/// is folded back into the transcript as local-command output. By default this
/// then starts a model turn so the assistant responds to the shell output;
/// users can set `respondToBashCommands=false` to keep it context-only.
/// Output is capped at 200 lines / ~8 KB so a `find /` doesn't fill the
/// chat scrollback. The TUI's renderer already truncates display to 20
/// lines (`render_user.rs::BashOutput`) but we keep the wire payload
/// modest to avoid bloating the JSONL transcript.
pub(super) async fn run_prompt_mode_bash(
    cwd: &std::path::Path,
    user_message_id: String,
    command: String,
    session: crate::session_runtime::SessionHandle,
    event_tx: mpsc::Sender<CoreEvent>,
    active_turn: Arc<Mutex<Option<ActiveTurn>>>,
    turn_done_tx: mpsc::Sender<uuid::Uuid>,
) {
    let runtime = &session;
    const MAX_OUTPUT_BYTES: usize = 8 * 1024;
    const MAX_OUTPUT_LINES: usize = 200;

    let mut executor = coco_shell::ShellExecutor::new(cwd);
    let exec_opts = coco_shell::ExecOptions::default();
    let mut command_failed_to_run = false;
    let (output, exit_code) = match executor.execute(&command, &exec_opts).await {
        Ok(result) => {
            let mut merged = String::new();
            if !result.stdout.is_empty() {
                merged.push_str(&result.stdout);
            }
            if !result.stderr.is_empty() {
                if !merged.is_empty() && !merged.ends_with('\n') {
                    merged.push('\n');
                }
                merged.push_str(&result.stderr);
            }
            (
                truncate_output(merged, MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES),
                result.exit_code,
            )
        }
        Err(err) => {
            command_failed_to_run = true;
            (format!("error: {err}"), -1)
        }
    };

    let should_respond = should_prompt_mode_bash_respond(&session) && !command_failed_to_run;

    // Push the local command into engine MessageHistory so the chat transcript
    // (TUI + SDK consumers + JSONL) records the bash invocation via the
    // standard `MessageAppended` event path. When the command is context-only,
    // prepend the carryover "DO NOT respond" caveat so a later model turn does
    // not comment on stale shell output.
    {
        let mut h = runtime.history().lock().await;
        let event_tx_opt = Some(event_tx.clone());
        if !should_respond {
            let caveat = coco_messages::create_meta_message(
                "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.</local-command-caveat>",
            );
            coco_query::history_sync::history_push_and_emit(&mut h, caveat, &event_tx_opt).await;
        }
        let msg = coco_messages::Message::System(coco_messages::SystemMessage::LocalCommand(
            coco_messages::SystemLocalCommandMessage {
                uuid: uuid::Uuid::new_v4(),
                command: command.clone(),
                output: output.clone(),
            },
        ));
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::BashCommandCompleted {
            user_message_id,
            output,
            exit_code,
        }))
        .await;

    if should_respond {
        let messages = {
            let h = runtime.history().lock().await;
            h.to_vec()
        };
        spawn_history_turn(messages, &session, &event_tx, &active_turn, &turn_done_tx).await;
    }
}

pub(super) fn should_prompt_mode_bash_respond(
    session: &crate::session_runtime::SessionHandle,
) -> bool {
    session
        .runtime_config()
        .settings
        .merged
        .respond_to_bash_commands
        .unwrap_or(true)
}
