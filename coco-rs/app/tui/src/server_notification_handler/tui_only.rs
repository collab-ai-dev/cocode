//! TUI-only handler.
//!
//! Handles [`TuiOnlyEvent`] — pane prompts, modals, picker data-ready
//! signals, compaction/speculation/cron toasts, and TUI-specific rewind
//! metadata. SDK and bridge consumers drop these events; only the TUI acts
//! on them.
//!
//! Complex event-specific logic (`RewindRowMetadataReady`,
//! `RewindRestorePreviewReady`, `RewindCompleted`) is extracted into
//! named helpers — `on_row_metadata_ready`,
//! `on_restore_preview_ready`, `on_rewind_completed` — so the match
//! arms stay scannable.

use coco_messages::SystemMessageLevel;
use coco_types::TuiOnlyEvent;

use crate::command::SystemPushKind;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::SavedSession;
use crate::state::SessionBrowserState;
use crate::state::SessionOption;
use crate::state::ui::Toast;

/// Dispatch a TUI-originated `Informational` system message directly
/// via `command_tx`. The engine handles `UserCommand::PushSystemMessage`
/// by calling `history_push_and_emit`, so the row surfaces in the
/// derived view via the normal `MessageAppended` round-trip.
fn enqueue_informational(
    _state: &mut AppState,
    level: SystemMessageLevel,
    title: &str,
    message: String,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) {
    if let Err(e) = command_tx.try_send(crate::command::UserCommand::PushSystemMessage {
        kind: SystemPushKind::Informational {
            level,
            title: title.to_string(),
            message,
        },
    }) {
        tracing::warn!(
            target: "coco_tui::system_push",
            title,
            error = ?e,
            "enqueue_informational: failed to dispatch PushSystemMessage",
        );
    }
}

/// Seed the editable "always allow" prefix for a shell-tool approval. Returns
/// `None` for non-shell tools or when no `command` string is present.
/// Delegates to `coco_permissions::shell_rules::editable_prefix_default`.
fn seed_prefix_input(
    tool_name: &str,
    original_input: Option<&serde_json::Value>,
    permission_suggestions: &[coco_types::PermissionUpdate],
) -> Option<crate::state::PrefixInputState> {
    if tool_name != coco_types::ToolName::Bash.as_str()
        && tool_name != coco_types::ToolName::PowerShell.as_str()
    {
        return None;
    }
    // Directory-boundary asks (out-of-dir bash write/redirect) carry an
    // AddDirectories suggestion, not a command rule. An editable command prefix
    // can't unblock them (the path gate runs before allow rules), so don't offer
    // one — let the AddDirectories suggestion drive the session/local rows.
    if permission_suggestions
        .iter()
        .any(|u| matches!(u, coco_types::PermissionUpdate::AddDirectories { .. }))
    {
        return None;
    }
    let command = original_input?.get("command")?.as_str()?;
    let value = coco_permissions::shell_rules::editable_prefix_from_suggestions_or_command(
        command,
        permission_suggestions,
    )?;
    Some(crate::state::PrefixInputState::new(value))
}

#[cfg(test)]
#[path = "tui_only.test.rs"]
mod tests;

pub(super) fn handle(
    state: &mut AppState,
    event: TuiOnlyEvent,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) -> bool {
    match event {
        TuiOnlyEvent::ApprovalRequired {
            request_id,
            tool_name,
            description,
            display_input,
            show_always_allow,
            choices,
            detail,
            permission_suggestions,
            original_input,
            cwd,
            worker_badge,
        } => {
            let detail = permission_detail_for_approval(&display_input, detail);
            let prefix_input = (choices.is_none() && show_always_allow)
                .then(|| {
                    seed_prefix_input(&tool_name, original_input.as_ref(), &permission_suggestions)
                })
                .flatten();
            state.ui.push_delayed_permission(
                crate::state::PermissionPromptState {
                    request_id,
                    tool_name,
                    description,
                    detail,
                    risk_level: None,
                    worker_badge,
                    // When the dialog is in choice mode, suppress the
                    // "Always Allow" affordance — the user is picking
                    // a one-shot action (e.g. keep/clear context), not
                    // declaring a durable rule.
                    show_always_allow: choices.is_none() && show_always_allow,
                    classifier_checking: false,
                    classifier_auto_approved: None,
                    choices,
                    selected_choice: 0,
                    display_input,
                    original_input,
                    cwd,
                    permission_suggestions,
                    explanation_visible: false,
                    explanation: crate::state::ExplainerFetch::NotFetched,
                    prefix_input,
                },
                std::time::Instant::now(),
            );
            true
        }
        TuiOnlyEvent::RewindRowMetadataReady { rows } => on_row_metadata_ready(state, rows),
        TuiOnlyEvent::RewindRestorePreviewReady { message_id, stats } => {
            on_restore_preview_ready(state, message_id, stats)
        }
        TuiOnlyEvent::RewindCompleted {
            target_message_id,
            files_changed,
        } => on_rewind_completed(state, target_message_id, files_changed),

        // === Question / elicitation / sandbox prompts ===
        TuiOnlyEvent::QuestionAsked { request_id, input } => {
            let questions = parse_question_items(&input);
            // Plan-mode gate for the Skip-interview footer item. Captured at
            // state construction so a mid-state mode flip doesn't change the
            // available footer items mid-flight.
            let is_in_plan_mode = state.session.permission_mode == coco_types::PermissionMode::Plan;
            state.ui.push_prompt(PanePromptState::Question(
                crate::state::QuestionPromptState {
                    request_id,
                    original_input: input,
                    questions,
                    current_question: crate::state::QuestionPage::Question(0),
                    focus_target: crate::state::QuestionFocusTarget::QuestionOption(0),
                    is_in_plan_mode,
                },
            ));
            true
        }
        TuiOnlyEvent::ElicitationRequested {
            request_id, server, ..
        } => {
            tracing::warn!(
                %request_id,
                %server,
                "dropping unsupported TUI elicitation request"
            );
            state.ui.add_toast(Toast::error(
                t!("toast.elicitation_unsupported", server = server.as_str()).to_string(),
            ));
            true
        }
        TuiOnlyEvent::SandboxApprovalRequired {
            request_id,
            operation,
        } => {
            state.ui.push_prompt(PanePromptState::SandboxPermission(
                crate::state::SandboxPermissionPromptState {
                    request_id,
                    description: operation,
                },
            ));
            true
        }
        TuiOnlyEvent::PermissionExplanationReady {
            request_id,
            explanation,
        } => {
            // Land the lazily-fetched explanation onto the still-active prompt.
            // If the prompt was dismissed/replaced before the fetch returned,
            // drop it silently.
            if let Some(PanePromptState::Permission(p)) =
                state.ui.interaction.active_prompt.as_mut()
                && p.request_id == request_id
            {
                p.explanation = match explanation {
                    Some(e) => crate::state::ExplainerFetch::Ready(e),
                    None => crate::state::ExplainerFetch::Unavailable,
                };
                return true;
            }
            false
        }

        // === Picker data-ready ===
        TuiOnlyEvent::PluginDataReady { plugins } => {
            state.session.available_plugins = plugins;
            true
        }
        TuiOnlyEvent::OutputStylesReady { styles } => {
            state.session.available_output_styles = styles;
            true
        }
        TuiOnlyEvent::AvailableCommandsRefreshed { commands } => {
            // Overwrite, not extend — the producer always sends the
            // full visible set so this trivially handles command
            // removals (e.g. plugin uninstall via /reload-plugins).
            state.session.available_commands = commands;
            // If the user is mid-`/` query, recompute the popup
            // against the new list so the next render reflects it
            // without waiting for another keystroke.
            crate::autocomplete::refresh_suggestions(state);
            true
        }
        TuiOnlyEvent::QueuedCommandEditReady {
            id: _,
            prompt,
            images,
        } => {
            state.ui.paste_manager.clear();
            let text = append_queued_edit_images(state, prompt, images);
            state.ui.input.set_text(&text);
            state.ui.input.textarea.set_cursor(text.len());
            true
        }
        TuiOnlyEvent::QueuedCommandsEditReady {
            ids: _,
            prompt,
            cursor,
            images,
        } => {
            let text = append_queued_edit_images(state, prompt, images);
            state.ui.input.set_text(&text);
            state.ui.input.textarea.set_cursor(cursor);
            true
        }
        TuiOnlyEvent::QueuedCommandEditUnavailable { id: _, reason } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.queued_edit_unavailable", reason = reason.as_str()).to_string(),
            ));
            true
        }
        TuiOnlyEvent::OpenSessionBrowser { sessions } => {
            let saved_sessions = sessions
                .into_iter()
                .map(|session| SavedSession {
                    id: session.session_id.clone(),
                    label: session.title.unwrap_or_else(|| session.session_id.clone()),
                    message_count: session.message_count,
                    created_at: session.created_at,
                    model: Some(session.model),
                })
                .collect::<Vec<_>>();
            let picker_sessions = saved_sessions
                .iter()
                .map(|session| SessionOption {
                    id: session.id.clone(),
                    label: session.label.clone(),
                    message_count: session.message_count,
                    created_at: session.created_at.clone(),
                })
                .collect();
            state.session.saved_sessions = saved_sessions;
            state
                .ui
                .show_modal(ModalState::SessionBrowser(SessionBrowserState {
                    sessions: picker_sessions,
                    filter: String::new(),
                    selected: 0,
                }));
            true
        }
        // === Compaction / speculation toasts ===
        TuiOnlyEvent::CompactionCircuitBreakerOpen { failures } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.compaction_breaker", failures = failures).to_string(),
            ));
            true
        }
        TuiOnlyEvent::MicroCompactionApplied { removed } => {
            state.ui.add_toast(Toast::info(
                t!("toast.micro_compaction", removed = removed).to_string(),
            ));
            true
        }
        TuiOnlyEvent::SessionMemoryCompactApplied { summary_tokens } => {
            state.ui.add_toast(Toast::info(
                t!("toast.session_memory_compacted", tokens = summary_tokens).to_string(),
            ));
            true
        }
        TuiOnlyEvent::SpeculativeRolledBack { reason } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.speculation_rolled_back", reason = reason.as_str()).to_string(),
            ));
            true
        }

        // === Memory extraction toasts ===
        TuiOnlyEvent::SessionMemoryExtractionStarted => {
            state
                .ui
                .add_toast(Toast::info(t!("toast.extracting_memories").to_string()));
            true
        }
        TuiOnlyEvent::SessionMemoryExtractionCompleted { extracted } => {
            state.ui.add_toast(Toast::success(
                t!("toast.memories_extracted_count", count = extracted).to_string(),
            ));
            true
        }
        TuiOnlyEvent::SessionMemoryExtractionFailed { error } => {
            state.ui.add_toast(Toast::error(
                t!("toast.memory_extract_failed_full", error = error.as_str()).to_string(),
            ));
            true
        }

        // === Cron toasts ===
        TuiOnlyEvent::CronJobDisabled { job_id: _, reason } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.cron_disabled_full", reason = reason.as_str()).to_string(),
            ));
            true
        }
        TuiOnlyEvent::CronJobsMissed { count } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.cron_missed_count", count = count).to_string(),
            ));
            true
        }

        // === Streaming tool display ===
        TuiOnlyEvent::ToolCallStreamStart { call_id, name } => {
            // Open a pre-queue streaming row so the activity strip shows the
            // tool (and its arguments, as they arrive) before it is queued.
            state.session.begin_tool_stream(call_id, name);
            true
        }
        TuiOnlyEvent::ToolCallDelta { call_id, delta } => {
            state.session.append_tool_stream_delta(&call_id, &delta);
            true
        }
        TuiOnlyEvent::ToolProgress { tool_use_id, data } => {
            let Some(tool) = state
                .session
                .tool_executions
                .iter_mut()
                .find(|t| t.call_id == tool_use_id)
            else {
                return false;
            };
            if let Some(desc) = data.get("description").and_then(|d| d.as_str()) {
                tool.description = Some(desc.to_string());
            }
            true
        }
        TuiOnlyEvent::ToolInterruptibilityChanged { interruptible } => {
            state.session.has_submit_interruptible_tool_in_progress = interruptible;
            true
        }
        TuiOnlyEvent::ToolExecutionAborted {
            tool_use_id: _,
            reason,
        } => {
            let reason = format!("{reason:?}");
            state.ui.add_toast(Toast::warning(
                t!("toast.tool_aborted_full", reason = reason.as_str()).to_string(),
            ));
            true
        }

        // === Slash-command result (tool-style `❯ /cmd` + `⎿ output`) ===
        TuiOnlyEvent::SlashCommandResult { name, args, text } => {
            // Render `❯ /cmd args` + `⎿ output` (tool-style) but keep it
            // `System` (transcript-only): these are user↔tool interactions
            // (/help, /model, /login, …), NOT conversation — the LLM must
            // not see them. Only Prompt/skill commands (which expand to
            // real model input) and /compact reach the model. Built here
            // (TUI owns the localized text) and round-tripped through engine
            // `MessageHistory` so transcript view, SDK, and JSONL converge.
            // No in-tree command sets `is_sensitive`, so redaction is a
            // no-op today (the builder still honors it for future commands).
            let messages = coco_messages::build_slash_command_messages(
                &name, &args, &text, /*is_sensitive*/ false,
            );
            if let Err(e) =
                command_tx.try_send(crate::command::UserCommand::PushSlashResult { messages })
            {
                tracing::warn!(
                    target: "coco_tui::system_push",
                    name,
                    error = ?e,
                    "SlashCommandResult: failed to dispatch PushSlashResult",
                );
            }
            true
        }
        // === `/context` full-color usage snapshot (inline in transcript) ===
        TuiOnlyEvent::OpenContextUsage { result } => {
            // `/context` prints `<ContextVisualization>` into the scrollback,
            // not a modal. Build the `❯ /context` echo + the typed system
            // snapshot and round-trip them through engine history so transcript
            // / SDK / JSONL converge (same path as /help).
            let messages = coco_messages::build_context_usage_messages(/*args*/ "", result);
            if let Err(e) =
                command_tx.try_send(crate::command::UserCommand::PushSlashResult { messages })
            {
                tracing::warn!(
                    target: "coco_tui::system_push",
                    error = ?e,
                    "OpenContextUsage: failed to dispatch PushSlashResult",
                );
            }
            true
        }
        // === Open the rewind picker state ===
        // Slash `/rewind` opens the bare picker. Internal preselect
        // flows use `TuiCommand::ShowRewindFor`.
        TuiOnlyEvent::OpenRewindPicker => on_open_rewind_picker(state, command_tx),
        // === Open the /memory file picker state ===
        // Entries are pre-built by the slash dispatcher (no extra state
        // lookup needed here). On select the TUI sends a command to the
        // CLI bridge; on cancel it emits a transcript line + toast.
        TuiOnlyEvent::OpenMemoryDialog { entries } => {
            if entries.is_empty() {
                state
                    .ui
                    .add_toast(Toast::warning(t!("dialog.memory_no_files").to_string()));
            } else {
                state.ui.show_modal(ModalState::MemoryDialog(
                    crate::state::MemoryDialogState::from_wire(entries),
                ));
            }
            true
        }
        // /skills — read-only overlay showing the skill catalog grouped
        // by source. Empty payload still opens the dialog so the user
        // sees the "no skills" hint instead of nothing happening.
        // Esc to close; no selection. The slash dispatcher pre-groups +
        // sorts + computes token estimates so the TUI is a pure projection.
        TuiOnlyEvent::OpenSkillsDialog { payload } => {
            state.ui.show_modal(ModalState::SkillsDialog(
                crate::state::SkillsDialogState::from_wire(payload),
            ));
            true
        }
        TuiOnlyEvent::OpenPluginDialog { payload } => {
            if let Some(ModalState::PluginDialog(existing)) = state.ui.modal.as_mut() {
                *existing = crate::state::PluginDialogState::from_wire(payload);
            } else {
                state.ui.show_modal(ModalState::PluginDialog(
                    crate::state::PluginDialogState::from_wire(payload),
                ));
            }
            true
        }
        // /agents — 2-tab overlay (Running + Library). Running tab
        // reads `SessionState.subagents` directly so the payload
        // only carries the Library list. The dialog renderer
        // groups by source at draw time.
        //
        // A repeated emit while the dialog is open (post-CRUD
        // refresh) updates the row list in place rather than
        // queueing a duplicate modal — that would land an invisible
        // refresh behind the visible (and now-stale) one.
        TuiOnlyEvent::OpenAgentsDialog { payload } => {
            let library = build_library_rows(payload);
            if let Some(ModalState::AgentsDialog(existing)) = state.ui.modal.as_mut() {
                existing.library = library;
                existing.snap_library_cursor();
            } else {
                state.ui.show_modal(ModalState::AgentsDialog(
                    crate::state::AgentsDialogState::new(library),
                ));
            }
            true
        }
        // `/permissions` — tabbed rule editor. The CLI re-emits this after
        // every persisted edit, so an already-open editor refreshes its
        // rule / directory data in place (preserving the focused tab +
        // cursors) rather than queueing a stale duplicate modal.
        TuiOnlyEvent::OpenPermissionsEditor { payload } => {
            if let Some(ModalState::PermissionsEditor(existing)) = state.ui.modal.as_mut() {
                existing.refresh_from_payload(payload);
            } else {
                state.ui.show_modal(ModalState::PermissionsEditor(
                    crate::state::PermissionsEditorState::from_payload(payload),
                ));
            }
            true
        }
        // `/add-dir` (no-arg) — open the interactive directory-input overlay.
        // An already-open instance is left as-is (the user is mid-edit).
        TuiOnlyEvent::OpenAddDirectory => {
            if !matches!(state.ui.modal.as_ref(), Some(ModalState::AddDirectory(_))) {
                state.ui.show_modal(ModalState::AddDirectory(
                    crate::state::AddDirectoryState::new(),
                ));
            }
            true
        }
        // `/export` (no-arg) — open the Markdown/JSON/Text format picker.
        // Confirm re-dispatches `/export <format>`, which the CLI runner
        // renders from the live `MessageHistory` and writes to disk.
        TuiOnlyEvent::OpenExport => {
            if !matches!(state.ui.modal.as_ref(), Some(ModalState::Export(_))) {
                state.ui.show_modal(ModalState::Export(
                    crate::state::surface_payloads::ExportState {
                        formats: vec![
                            crate::state::ExportFormat::Markdown,
                            crate::state::ExportFormat::Json,
                            crate::state::ExportFormat::Text,
                        ],
                        selected: 0,
                    },
                ));
            }
            true
        }
        // `/skills` dialog Enter result — CLI bridge has finished
        // (or failed) the SettingsWriter round-trip + RuntimeConfig
        // republish + CommandRegistry rebuild. Toast generation
        // lives here (not in the CLI handler) so the `t!` macro can
        // pull the localized strings — the i18n catalog is anchored
        // at this crate root and can't be reached from `coco-cli`.
        //
        // The Enter handler stashed `total_edits` on
        // `UiState.pending_skills_save_edits` before dispatch; we
        // read + clear it here so the count never crosses the wire
        // (TUI owns display data, CLI doesn't).
        TuiOnlyEvent::SkillOverridesSaved { result } => {
            let total_edits = state.ui.pending_skills_save_edits.take().unwrap_or(0);
            let text = format_skill_overrides_save_toast(result, total_edits);
            state.ui.add_toast(Toast::info(text));
            true
        }
        // /copy [N] — branch into either direct clipboard write or the
        // CopyPicker modal based on `copy_full_response` + presence of
        // code blocks.
        TuiOnlyEvent::CopyCommandRequested { args } => {
            if let Some(message) = crate::copy::handle_copy_command(state, &args) {
                enqueue_informational(state, SystemMessageLevel::Info, "", message, command_tx);
            }
            true
        }
        TuiOnlyEvent::MemoryFileOpened { path } => {
            let text = t!("toast.memory_opened", path = path.as_str()).to_string();
            enqueue_informational(
                state,
                SystemMessageLevel::Info,
                "",
                text.clone(),
                command_tx,
            );
            state.ui.add_toast(Toast::info(text));
            true
        }
        TuiOnlyEvent::MemoryFileOpenFailed { path: _, error } => {
            let text = t!("toast.memory_open_failed", error = error.as_str()).to_string();
            enqueue_informational(
                state,
                SystemMessageLevel::Warning,
                "",
                text.clone(),
                command_tx,
            );
            state.ui.add_toast(Toast::warning(text));
            true
        }
        TuiOnlyEvent::PlanFileOpened { path } => {
            let text = t!("toast.plan_opened", path = path.as_str()).to_string();
            enqueue_informational(
                state,
                SystemMessageLevel::Info,
                "",
                text.clone(),
                command_tx,
            );
            state.ui.add_toast(Toast::info(text));
            true
        }
        TuiOnlyEvent::PlanFileOpenFailed { path: _, error } => {
            let text = t!("toast.plan_open_failed", error = error.as_str()).to_string();
            enqueue_informational(
                state,
                SystemMessageLevel::Warning,
                "",
                text.clone(),
                command_tx,
            );
            state.ui.add_toast(Toast::warning(text));
            true
        }
        TuiOnlyEvent::ExternalEditorPrepare { .. } => false,
        TuiOnlyEvent::PromptEditorCompleted { content, modified } => {
            state.ui.input.set_text(&content);
            state.ui.input.textarea.set_cursor(content.len());
            let text = if modified {
                t!("toast.prompt_editor_updated")
            } else {
                t!("toast.prompt_editor_unchanged")
            }
            .to_string();
            state.ui.add_toast(Toast::info(text));
            true
        }
        TuiOnlyEvent::PromptEditorFailed { error } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.prompt_editor_failed", error = error.as_str()).to_string(),
            ));
            true
        }
        TuiOnlyEvent::BashCommandCompleted {
            user_message_id: _,
            output: _,
            exit_code: _,
        } => {
            // Visible bash output flows through the standard engine
            // path: `run_prompt_mode_bash` pushes a single
            // `SystemMessage::LocalCommand { command, output }` via
            // `history_push_and_emit` before this event fires, so the
            // transcript view already shows the result by the time we
            // reach here. The event is kept for observability (the
            // `emit.rs` event-name map still surfaces it on the wire)
            // but no longer drives TUI transcript writes.
            false
        }
        TuiOnlyEvent::OpenModelPicker => {
            crate::update::show::cycle_model(state);
            true
        }
        TuiOnlyEvent::OpenSettings => {
            crate::update::show::settings(state);
            true
        }
        TuiOnlyEvent::OpenThemePicker => {
            crate::update::show::open_theme_picker(state);
            true
        }
        TuiOnlyEvent::SlashCommandStatus { name, args, kind } => {
            use coco_types::SlashCommandStatusKind;
            // Render dispatcher statuses inline tool-style (`❯ /cmd args` +
            // `⎿ …`) so they match the success path and a real tool result,
            // rather than a detached system note. `Failed` is the only true
            // error → stderr (error color); the rest are informational → stdout
            // (dim), keyed off the same builders the success path uses.
            let (is_error, text) = match kind {
                SlashCommandStatusKind::NoHandler => (
                    false,
                    t!("slash.status.no_handler", name = name.as_str()).to_string(),
                ),
                SlashCommandStatusKind::Failed { error } => (
                    true,
                    t!(
                        "slash.status.failed",
                        name = name.as_str(),
                        error = error.as_str()
                    )
                    .to_string(),
                ),
                SlashCommandStatusKind::EmptyPrompt => (
                    false,
                    t!("slash.status.empty_prompt", name = name.as_str()).to_string(),
                ),
                SlashCommandStatusKind::DialogPending { dialog_kind } => (
                    false,
                    t!(
                        "slash.status.dialog_pending",
                        name = name.as_str(),
                        dialog_kind = dialog_kind.as_str()
                    )
                    .to_string(),
                ),
                SlashCommandStatusKind::PermissionsUsageAllow => {
                    (false, t!("slash.permissions.usage_allow").to_string())
                }
                SlashCommandStatusKind::PermissionsUsageDeny => {
                    (false, t!("slash.permissions.usage_deny").to_string())
                }
            };
            let messages = if is_error {
                coco_messages::build_slash_command_error_messages(&name, &args, &text)
            } else {
                coco_messages::build_slash_command_messages(
                    &name, &args, &text, /*is_sensitive*/ false,
                )
            };
            if let Err(e) =
                command_tx.try_send(crate::command::UserCommand::PushSlashResult { messages })
            {
                tracing::warn!(
                    target: "coco_tui::system_push",
                    name,
                    error = ?e,
                    "SlashCommandStatus: failed to dispatch PushSlashResult",
                );
            }
            true
        }
    }
}

fn permission_detail_for_approval(
    display_input: &coco_types::PermissionDisplayInput,
    detail: Option<coco_types::PermissionRequestDetail>,
) -> crate::state::PermissionDetail {
    if let Some(coco_types::PermissionRequestDetail::ExitPlanMode {
        outcome,
        plan,
        plan_file_path,
        allowed_prompts,
    }) = detail
    {
        return crate::state::PermissionDetail::ExitPlanMode {
            outcome,
            plan,
            plan_file_path,
            allowed_prompts,
        };
    }
    crate::state::PermissionDetail::Generic {
        input_preview: display_input.as_display_str().to_string(),
    }
}

fn append_queued_edit_images(
    state: &mut AppState,
    mut text: String,
    images: Vec<coco_types::QueuedCommandEditImage>,
) -> String {
    for image in images {
        match base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            image.data_base64.as_bytes(),
        ) {
            Ok(bytes) => {
                let pill = state
                    .ui
                    .paste_manager
                    .add_image_data(bytes, image.media_type);
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&pill);
            }
            Err(e) => {
                state.ui.add_toast(Toast::warning(
                    t!(
                        "toast.queued_edit_image_failed",
                        error = e.to_string().as_str()
                    )
                    .to_string(),
                ));
            }
        }
    }
    text
}

/// Handle `TuiOnlyEvent::OpenRewindPicker`.
///
/// Opens the bare picker and fires one batched per-row metadata
/// command. The CLI driver responds with a single
/// `RewindRowMetadataReady` carrying `(can_restore, +X -Y, files)`
/// for every row without overflowing the bounded TUI command channel.
fn on_open_rewind_picker(
    state: &mut AppState,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) -> bool {
    let picker_id = uuid::Uuid::new_v4();
    tracing::info!(
        target: "rewind::tui",
        %picker_id,
        "OpenRewindPicker received",
    );

    let rewind = crate::update_rewind::build_rewind_state(state);

    // No empty-rewind early-out: `build_rewind_state` always appends
    // the synthetic `(current)` row, so `rewind.messages` is never
    // empty. The renderer's `picker_is_empty` check renders an inline
    // "Nothing to rewind to yet." when only the synthetic row exists.
    // The picker still opens so Esc has a well-defined target.

    tracing::info!(
        target: "rewind::tui",
        %picker_id,
        phase = ?rewind.phase,
        row_count = rewind.messages.len(),
        preselected = rewind.preselected,
        file_history_enabled = rewind.file_history_enabled,
        "rewind picker opened",
    );

    let row_uuids = crate::update::show::preload_diff_stats_targets(&rewind);
    state.ui.show_modal(ModalState::Rewind(rewind));
    if !row_uuids.is_empty() {
        let message_ids = row_uuids.into_iter().map(|id| id.to_string()).collect();
        if let Err(e) =
            command_tx.try_send(crate::command::UserCommand::RequestDiffStatsBatch { message_ids })
        {
            tracing::warn!(
                target: "rewind::tui",
                error = ?e,
                "preload RequestDiffStatsBatch dropped; rows will resolve on selection",
            );
        }
    }
    true
}

/// Apply per-row metadata batch (`RewindRowMetadataReady`).
///
/// One in-place pass per row keyed by `message_id`; `metadata: None`
/// signals that the checkpoint has no code restore (renders "⚠ No code
/// restore").
fn on_row_metadata_ready(state: &mut AppState, rows: Vec<coco_types::RewindRowMetadata>) -> bool {
    let Some(ModalState::Rewind(r)) = state.ui.modal.as_mut() else {
        return false;
    };
    for entry in rows {
        let Ok(uuid) = uuid::Uuid::parse_str(&entry.message_id) else {
            tracing::warn!(
                target: "rewind::tui",
                row_message_id = entry.message_id,
                "RewindRowMetadataReady carried a non-uuid id; skipping row",
            );
            continue;
        };
        let Some(row) = r.messages.iter_mut().find(|m| m.message_id == uuid) else {
            continue;
        };
        match entry.metadata {
            Some(payload) => {
                row.can_restore_code = Some(true);
                row.diff_stats = Some(crate::state::DiffStatsPreview {
                    insertions: payload.insertions,
                    deletions: payload.deletions,
                    file_paths: payload.file_paths,
                });
            }
            None => {
                row.can_restore_code = Some(false);
                row.diff_stats = None;
            }
        }
    }
    true
}

/// Apply the single selected-checkpoint restore preview
/// (`RewindRestorePreviewReady`). Drives the `RestoreOptions` phase
/// for the selected row only; per-row labels are unaffected.
///
/// `stats == None` signals that code restore is unavailable for this
/// checkpoint.
fn on_restore_preview_ready(
    state: &mut AppState,
    message_id: String,
    stats: Option<coco_types::RewindDiffStatsPayload>,
) -> bool {
    let Ok(stats_uuid) = uuid::Uuid::parse_str(&message_id) else {
        tracing::warn!(
            target: "rewind::tui",
            stats_message_id = message_id,
            "RewindRestorePreviewReady carried a non-uuid id; dropping",
        );
        return false;
    };
    let Some(ModalState::Rewind(r)) = state.ui.modal.as_mut() else {
        return false;
    };
    let selected_id = r.messages.get(r.selected as usize).map(|m| m.message_id);
    if selected_id != Some(stats_uuid) {
        // The user navigated away before the preview resolved.
        // `handle_rewind_confirm` re-requests on the new row.
        return true;
    }

    let prior_option = r.available_options.get(r.option_selected as usize).cloned();

    match stats {
        Some(payload) => {
            let preview = crate::state::DiffStatsPreview {
                insertions: payload.insertions,
                deletions: payload.deletions,
                file_paths: payload.file_paths,
            };
            r.has_file_changes = !preview.file_paths.is_empty();
            r.diff_stats = Some(preview);
            r.diff_stats_message_id = Some(stats_uuid);
            r.available_options = crate::state::rewind::build_restore_options(
                r.file_history_enabled,
                r.has_file_changes,
                r.allow_summarize_up_to,
            );
            // Also flip the row badge — picker may re-open at this row
            // and the metadata batch may not have arrived yet.
            if let Some(row) = r.messages.iter_mut().find(|m| m.message_id == stats_uuid) {
                row.can_restore_code = Some(true);
            }
        }
        None => {
            r.has_file_changes = false;
            r.diff_stats = None;
            r.diff_stats_message_id = Some(stats_uuid);
            r.available_options = crate::state::rewind::build_restore_options(
                r.file_history_enabled,
                /*has_file_changes*/ false,
                r.allow_summarize_up_to,
            );
            if let Some(row) = r.messages.iter_mut().find(|m| m.message_id == stats_uuid) {
                row.can_restore_code = Some(false);
                row.diff_stats = None;
            }
        }
    }

    // Reposition the option cursor so a rebuild that prepends
    // `Both` / `CodeOnly` doesn't silently move the user's focus to a
    // different RestoreType.
    if let Some(prior) = prior_option {
        if let Some(new_idx) = r
            .available_options
            .iter()
            .position(|opt| opt.variant_eq(&prior))
        {
            r.option_selected = new_idx as i32;
        } else {
            r.option_selected = 0;
        }
    } else {
        r.option_selected = 0;
    }
    true
}

fn on_rewind_completed(
    state: &mut AppState,
    target_message_id: String,
    files_changed: i32,
) -> bool {
    let mut restored_permission_mode = None;
    let mut restored_input_text = None;

    let mut restored_image: Option<(Vec<u8>, String)> = None;
    if !target_message_id.is_empty() {
        // Search the engine-authoritative cell list for the rewound
        // message. UI restoration reads `cell.source` directly.
        let cells = state.session.transcript.cells();
        if let Some(target_cell) = cells
            .iter()
            .find(|c| c.message_uuid.to_string() == target_message_id)
        {
            if let coco_messages::Message::User(u) = target_cell.source.as_ref() {
                restored_permission_mode = u.permission_mode;
            }
            // Strip IDE-injected context tags so the restored prompt doesn't
            // leak `<ide_opened_file>` / `<ide_selection>` blocks.
            let raw = match &target_cell.kind {
                crate::transcript::cells::CellKind::UserText { text } => text.as_str(),
                _ => "",
            };
            let stripped = crate::update_rewind::strip_ide_context_tags(raw);
            restored_input_text = Some(stripped).filter(|s| !s.is_empty());
            // Restore pasted images from the rewound message. Pasted images
            // live on `UserContentPart::File` (`image/*` media type) as
            // bytes/base64 — `to_bytes()` covers both; a remote URL cannot
            // be re-attached as a paste pill (pills without bytes are dropped
            // at submit), so it is skipped. Only the first image is surfaced.
            if let coco_messages::Message::User(u) = target_cell.source.as_ref()
                && let coco_messages::LlmMessage::User { content, .. } = &u.message
            {
                for part in content {
                    if let coco_messages::UserContent::File(f) = part
                        && f.media_type.starts_with("image/")
                        && let Some(data) = f.data.as_data()
                        && let Some(bytes) = data.to_bytes()
                    {
                        restored_image = Some((bytes, f.media_type.clone()));
                        break;
                    }
                }
            }
        }
    }

    // The engine emits `MessageTruncated` after this handler — that
    // event truncates `state.session.transcript`, the single source
    // of truth for rendering.

    if let Some(mode) = restored_permission_mode {
        state.session.permission_mode = mode;
    }

    if let Some(text) = restored_input_text {
        state.ui.input.textarea.set_text(&text);
        let eol = state.ui.input.textarea.end_of_current_line();
        state.ui.input.textarea.set_cursor(eol);
    }

    // Rotate conversation_id on truncate so the next request breaks
    // any prior cache key.
    if !target_message_id.is_empty() {
        state.session.conversation_id = Some(uuid::Uuid::new_v4().to_string());
    }

    // Clear the prompt-suggestion belt — stale suggestions from
    // earlier turns are no longer valid in the rewound conversation.
    state.session.prompt_suggestions.clear();

    // Paste buffer handling — rebuild from the rewound message's image
    // blocks. Each user message carries at most one image; if present,
    // re-attach it WITH its bytes (a path-only pill stores
    // `image_bytes: None` and `resolve_structured` silently drops it at
    // submit); otherwise clear any leftover paste-buffer state so it
    // doesn't leak into the new turn.
    state.ui.paste_manager.clear();
    if let Some((bytes, mime)) = restored_image {
        state.ui.paste_manager.add_image_data(bytes, mime);
    }

    state.ui.scroll_offset = 0;
    state.ui.user_scrolled = false;
    state.ui.dismiss_modal();

    let msg = if files_changed > 0 {
        t!("toast.rewound_checkpoint", count = files_changed).to_string()
    } else {
        t!("toast.conversation_rewound_checkpoint").to_string()
    };
    state.ui.add_toast(Toast::success(msg));
    true
}

/// Parse the AskUserQuestion tool input dict into rich
/// `QuestionItem`s the state can render.
///
/// Tolerant parser — missing/optional fields use defaults so a
/// model that emits a partial schema still produces a usable state
/// rather than a blank screen.
fn parse_question_items(input: &serde_json::Value) -> Vec<crate::state::QuestionItem> {
    // The tool schema dropped its hard `maxItems` caps so a weak model that
    // over-generates doesn't hard-fail validation (which retry-loops and
    // flickers the bottom bar). Enforce the intended cap here on display: keep
    // the first 4 questions and the first 4 options per question.
    const MAX_QUESTIONS: usize = 4;
    const MAX_OPTIONS: usize = 4;
    let Some(arr) = input.get("questions").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .take(MAX_QUESTIONS)
        .map(|q| {
            let header = str_field(q, "header").to_string();
            let question = str_field(q, "question").to_string();
            let multi_select = q
                .get("multiSelect")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let options: Vec<crate::state::QuestionOption> = q
                .get("options")
                .and_then(serde_json::Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .take(MAX_OPTIONS)
                        .map(|o| crate::state::QuestionOption {
                            label: str_field(o, "label").to_string(),
                            description: str_field(o, "description").to_string(),
                            preview: o
                                .get("preview")
                                .and_then(serde_json::Value::as_str)
                                .map(String::from),
                        })
                        .collect()
                })
                .unwrap_or_default();
            crate::state::QuestionItem {
                header,
                question,
                options,
                multi_select,
                selected: None,
                checked: Vec::new(),
                other_input: crate::state::OtherInputState::default(),
            }
        })
        .collect()
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(serde_json::Value::as_str).unwrap_or("")
}

/// Render a localized toast for the `/skills` dialog Enter result.
/// `total_edits` is the count the dialog computed at dispatch (read
/// from `UiState.pending_skills_save_edits`); only consumed on the
/// Ok branch.
fn format_skill_overrides_save_toast(
    result: coco_types::SkillOverridesSaveResult,
    total_edits: usize,
) -> String {
    use coco_types::SkillOverridesSaveResult;
    match result {
        SkillOverridesSaveResult::Ok if total_edits == 0 => {
            t!("dialog.skills_save_no_changes").to_string()
        }
        SkillOverridesSaveResult::Ok => {
            let noun = if total_edits == 1 {
                t!("dialog.skills_override_noun_singular")
            } else {
                t!("dialog.skills_override_noun_plural")
            };
            t!(
                "dialog.skills_save_updated",
                n = total_edits.to_string().as_str(),
                noun = noun.as_ref()
            )
            .to_string()
        }
        SkillOverridesSaveResult::Err { message, .. } => {
            t!("dialog.skills_save_failed", error = message.as_str()).to_string()
        }
    }
}

/// Translate an [`coco_types::AgentsDialogPayload`] into the flat
/// row list rendered by the Library tab.
///
/// Source ordering: User → Project → Local (collapsed into Project until
/// coco-rs distinguishes worktree-local from repo-root project) → Managed
/// → Plugin → Flag → Built-in. Empty groups are omitted; built-in always
/// renders last.
fn build_library_rows(payload: coco_types::AgentsDialogPayload) -> Vec<crate::state::LibraryRow> {
    use crate::state::LibraryRow;
    use coco_types::AgentSource;
    let mut rows = vec![LibraryRow::CreateNew];

    // Group label + ordering: Local is intentionally grouped with Project
    // until the loader gains worktree-local distinction
    // (see `state/agents_dialog.rs` doc).
    let group_order: &[(AgentSource, &str)] = &[
        (AgentSource::UserSettings, "dialog.agents_group_user"),
        (AgentSource::ProjectSettings, "dialog.agents_group_project"),
        (AgentSource::PolicySettings, "dialog.agents_group_policy"),
        (AgentSource::Plugin, "dialog.agents_group_plugin"),
        (AgentSource::FlagSettings, "dialog.agents_group_flag"),
        (AgentSource::BuiltIn, "dialog.agents_group_builtin"),
    ];
    for (source, label_key) in group_order {
        let group_entries: Vec<&coco_types::AgentsDialogEntry> = payload
            .entries
            .iter()
            .filter(|e| e.source == *source)
            .collect();
        if group_entries.is_empty() {
            continue;
        }
        let label = t!(*label_key).to_string();
        rows.push(LibraryRow::SourceHeader { label });
        let is_builtin = matches!(*source, AgentSource::BuiltIn);
        for entry in group_entries {
            rows.push(LibraryRow::Agent {
                name: entry.name.clone(),
                description: if entry.description.is_empty() {
                    None
                } else {
                    Some(entry.description.clone())
                },
                source: entry.source,
                color: entry.color,
                is_builtin,
                is_overridden: entry.is_overridden,
                // Running count comes from the live `SessionState.subagents`
                // at render time, not the wire — TUI computes it per
                // frame so reloads stay cheap.
                running_count: 0,
                source_path: entry.source_path.clone(),
            });
        }
    }
    rows
}
