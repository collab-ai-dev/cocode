use super::*;

/// Emit a `TuiOnlyEvent::SlashCommandResult` so the TUI appends a
/// system-role chat message carrying handler-rendered content (verbatim,
/// no translation).
pub(super) async fn emit_slash_text(
    event_tx: &mpsc::Sender<CoreEvent>,
    session_id: &coco_types::SessionId,
    name: &str,
    args: &str,
    text: &str,
) {
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SlashCommandResult {
            session_id: session_id.clone(),
            name: name.to_string(),
            args: args.to_string(),
            text: text.to_string(),
        }))
        .await;
}

pub(super) async fn dispatch_context(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) -> SlashOutcome {
    let session_id = session.session_id();
    let Some(target) = session_target_for(local_app_server_bridge, session_id) else {
        emit_slash_status(
            event_tx,
            session_id,
            "context",
            "failed",
            SlashCommandStatusKind::Failed {
                error: "full session attachment is no longer available".to_string(),
            },
        )
        .await;
        return SlashOutcome::Handled;
    };
    match local_app_server_bridge
        .client()
        .context_usage(local_app_server_bridge.handler(), target)
        .await
    {
        Ok(result) => {
            let _ = event_tx
                .send(CoreEvent::Tui(TuiOnlyEvent::OpenContextUsage {
                    session_id: session_id.clone(),
                    result,
                }))
                .await;
        }
        Err(e) => {
            emit_slash_status(
                event_tx,
                session_id,
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
        coco_agent_host::app_session::AppSessionHandle,
    >,
    handler: coco_agent_host::app_server_host::AppServerHostHandler,
) {
    let runtime = session;
    let plan_exited = runtime.has_exited_plan_mode().await;
    let plan_text = runtime.unscoped_session_plan_text(session_id);
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
    let truncate = match runtime.truncate_history_at_user_message(message_id).await {
        Ok(result) => result,
        Err(history_len) => {
            // Auto-restore is fire-and-forget; if the target uuid is gone
            // (e.g. a compaction wiped it between TUI dispatch and engine
            // handler), we'd rather skip silently than panic. `warn` so
            // ops can correlate "auto-restore quietly did nothing" with
            // an upstream truncation race.
            tracing::warn!(
                target: "coco_agent_host::auto_truncate",
                message_id,
                history_len,
                "AutoTruncate target message not found in history (likely raced with compaction)",
            );
            return;
        }
    };
    tracing::info!(
        target: "coco_agent_host::auto_truncate",
        message_id,
        keep_count = truncate.keep_count,
        removed = truncate.removed,
        "AutoTruncate applied",
    );
    coco_otel::events::emit_conversation_rewind(
        truncate.pre_count as i64,
        truncate.keep_count as i64,
        truncate.removed as i64,
        truncate.keep_count as i64,
    );
    let _ = event_tx
        .send(CoreEvent::Protocol(ServerNotification::MessageTruncated {
            keep_count: truncate.keep_count as i64,
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
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
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
        && runtime.file_history_enabled()
    {
        if let Err(error) = local_app_server_bridge
            .activate_existing_full_session(session.session_id().clone(), None)
        {
            tracing::warn!(%error, "rewind could not activate local AppServer session");
            return;
        }
        match local_app_server_bridge
            .client()
            .rewind_files(
                local_app_server_bridge.handler(),
                coco_types::RewindFilesParams {
                    target: session_target(local_app_server_bridge),
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
        match runtime.truncate_history_at_user_message(message_id).await {
            Ok(truncate) => {
                messages_removed = i32::try_from(truncate.removed).unwrap_or(i32::MAX);
                tracing::info!(
                    target: "coco_agent_host::rewind",
                    message_id,
                    keep_count = truncate.keep_count,
                    messages_removed,
                    files_changed,
                    "Explicit rewind: truncated history",
                );
                coco_otel::events::emit_conversation_rewind(
                    truncate.pre_count as i64,
                    truncate.keep_count as i64,
                    truncate.removed as i64,
                    truncate.keep_count as i64,
                );
                keep_count_to_emit = Some(truncate.keep_count as i64);
            }
            Err(history_len) => {
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
    use coco_messages::PartialCompactDirection;
    use coco_tui::state::RestoreType;

    let (direction, feedback) = match restore_type {
        RestoreType::SummarizeFrom { feedback } => {
            (PartialCompactDirection::Newest, feedback.clone())
        }
        RestoreType::SummarizeUpTo { feedback } => {
            (PartialCompactDirection::Oldest, feedback.clone())
        }
        _ => return,
    };

    let outcome = coco_agent_host::session_compaction::run_summarize_rewind(
        session,
        message_id,
        direction,
        feedback,
        Some(event_tx.clone()),
    )
    .await;

    match outcome {
        coco_agent_host::session_compaction::SummarizeRewindOutcome::Applied => {
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
        coco_agent_host::session_compaction::SummarizeRewindOutcome::TargetMissing => {
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
        }
        coco_agent_host::session_compaction::SummarizeRewindOutcome::Failed => {
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
        coco_agent_host::app_session::AppSessionHandle,
    >,
    handler: coco_agent_host::app_server_host::AppServerHostHandler,
) {
    tokio::spawn(async move {
        if session.has_persisted_title().await {
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
        let session_id = session.session_id().clone();
        if session.has_persisted_title().await {
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
/// Persist skill override edits through host session controls and forward the
/// resulting UI notifications. The localized toast stays in `coco-tui`.
pub(super) async fn handle_write_skill_overrides(
    session: &crate::session_runtime::SessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    patch: serde_json::Value,
    runtime_publisher: Option<&Arc<coco_config::RuntimePublisher>>,
    cwd: &std::path::Path,
    flag_settings: Option<&std::path::Path>,
) {
    let update = coco_agent_host::session_controls::write_skill_overrides(
        session,
        patch,
        runtime_publisher,
        cwd,
        flag_settings,
    )
    .await;
    if let Some(commands) = update.commands {
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
                commands,
            }))
            .await;
    }

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::SkillOverridesSaved {
            result: update.result,
        }))
        .await;
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
            insertion_offset: i64::try_from(img.insertion_offset).unwrap_or(i64::MAX),
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
            insertion_offset: image.insertion_offset,
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
    response_turn_tx: mpsc::Sender<Vec<std::sync::Arc<coco_messages::Message>>>,
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

    let should_respond =
        coco_agent_host::session_controls::should_respond_to_bash_commands(&session)
            && !command_failed_to_run;

    let history_after_append =
        coco_agent_host::session_messages::append_local_command_to_history_and_emit(
            runtime,
            event_tx.clone(),
            &command,
            &output,
            should_respond,
        )
        .await;

    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::BashCommandCompleted {
            user_message_id,
            output,
            exit_code,
        }))
        .await;

    if should_respond && response_turn_tx.send(history_after_append).await.is_err() {
        tracing::warn!("prompt-mode bash response turn channel closed");
    }
}
