//! Modal-surface pane: per-modal behavior behind one routing layer.
//!
//! `UiState` owns the modal slot; this module owns modal behavior. The update
//! layer routes bottom-pane prompts first where required and falls through here.

pub(crate) mod add_directory;
pub(crate) mod model_picker;
pub(crate) mod permissions_editor;
pub(crate) mod settings;
pub(crate) mod team_roster;

use coco_types::PermissionMode;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::SystemPushKind;
use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ExportFormat;
use crate::state::ModalState;
use crate::state::SessionBrowserState;
use crate::state::SessionOption;
use crate::state::ui::Toast;
use crate::update_rewind;
use coco_tui_ui::constants;

pub(crate) fn picker_map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Home => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::End => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Up if shift => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::Down if shift => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Backspace => Some(TuiCommand::SurfaceFilterBackspace),
        KeyCode::Char('c') if ctrl => Some(TuiCommand::Cancel),
        KeyCode::Char('p') if ctrl => Some(TuiCommand::SurfacePrev),
        KeyCode::Char('n') if ctrl => Some(TuiCommand::SurfaceNext),
        KeyCode::Char(c) => Some(TuiCommand::SurfaceFilter(c)),
        _ => None,
    }
}

pub(crate) fn scrollable_map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(TuiCommand::Cancel),
        KeyCode::Up | KeyCode::Char('k') => Some(TuiCommand::SurfacePrev),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiCommand::SurfaceNext),
        KeyCode::PageUp => Some(TuiCommand::PageUp),
        KeyCode::PageDown => Some(TuiCommand::PageDown),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        _ => None,
    }
}

pub(crate) fn transcript_map_key(key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(TuiCommand::Cancel),
        KeyCode::Up | KeyCode::Char('k') => Some(TuiCommand::TranscriptScrollLines(-1)),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiCommand::TranscriptScrollLines(1)),
        KeyCode::Home => Some(TuiCommand::TranscriptJumpStart),
        KeyCode::End => Some(TuiCommand::TranscriptJumpEnd),
        KeyCode::PageUp => Some(TuiCommand::TranscriptPage(-1)),
        KeyCode::PageDown => Some(TuiCommand::TranscriptPage(1)),
        KeyCode::Tab => Some(TuiCommand::TranscriptSelectNext),
        KeyCode::Enter => Some(TuiCommand::TranscriptToggleCell),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        _ => None,
    }
}

pub(crate) async fn approve(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    match state.ui.modal.as_ref() {
        Some(ModalState::BypassPermissions(_)) => {
            if !state.session.bypass_permissions_available {
                state.ui.dismiss_modal();
                return;
            }
            state.session.permission_mode = PermissionMode::BypassPermissions;
            let _ = command_tx
                .send(UserCommand::SetPermissionMode {
                    mode: PermissionMode::BypassPermissions,
                })
                .await;
            state.ui.dismiss_modal();
        }
        Some(ModalState::PluginHint(ph)) => {
            let response = ph.selected_response();
            let plugin_id = ph.plugin_id.clone();
            let plugin_name = ph.plugin_name.clone();
            apply_plugin_hint_response(state, command_tx, response, &plugin_id, &plugin_name).await;
            state.ui.dismiss_modal();
        }
        Some(ModalState::Trust(_) | ModalState::WorktreeExit(_)) => {
            state.ui.dismiss_modal();
        }
        _ => {
            state.ui.dismiss_modal();
        }
    }
}

async fn apply_plugin_hint_response(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
    response: crate::state::PluginHintResponse,
    plugin_id: &str,
    plugin_name: &str,
) {
    use crate::state::PluginHintResponse;

    coco_plugins::mark_hint_plugin_shown(plugin_id);

    match response {
        PluginHintResponse::Install => {
            if let Ok(name) = crate::state::SlashCommandName::new("plugin") {
                let _ = command_tx
                    .send(UserCommand::ExecuteSlashCommand {
                        name,
                        args: format!("install {plugin_id}"),
                    })
                    .await;
            }
            state.ui.add_toast(crate::state::ui::Toast::info(
                t!("toast.plugin_hint_installing", name = plugin_name).to_string(),
            ));
        }
        PluginHintResponse::Disable => {
            coco_plugins::disable_hint_recommendations();
            state.ui.add_toast(crate::state::ui::Toast::info(
                t!("toast.plugin_hint_disabled").to_string(),
            ));
        }
        PluginHintResponse::Dismiss => {}
    }
}

pub(crate) async fn deny(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    match state.ui.modal.as_ref() {
        Some(ModalState::BypassPermissions(_)) => {
            state.ui.dismiss_modal();
        }
        _ => close_modal_with_feedback(state, command_tx).await,
    }
}

/// How a picker/dialog reports being closed with Esc. Every picker leaves a
/// transcript trace of what closed.
enum PickerDismiss {
    /// Picker opened by a slash command -> render `❯ /<name>` + `⎿ <message>`,
    /// matching the command's own confirm feedback (e.g. theme "Theme set to").
    Slash {
        name: &'static str,
        message: &'static str,
    },
    /// Keybinding-only overlay (no slash command) -> a standalone system line,
    /// with no misleading `/cmd` echo.
    System { message: &'static str },
}

/// Dismiss feedback for a modal closed via Esc. `None` for prompt-style and
/// viewer modals that own their own decline UX. Wording used for toast
/// messages where a counterpart exists (`Theme picker dismissed`, `Skills dialog dismissed`,
/// `Cancelled memory editing`, ...).
fn picker_dismiss(modal: &ModalState) -> Option<PickerDismiss> {
    use ModalState as M;
    use PickerDismiss::Slash;
    use PickerDismiss::System;
    Some(match modal {
        M::Help => Slash {
            name: "help",
            message: "Help dialog dismissed",
        },
        M::ModelPicker(_) => Slash {
            name: "model",
            message: "Model picker dismissed",
        },
        M::ThemePicker(_) => Slash {
            name: "theme",
            message: "Theme picker dismissed",
        },
        M::SkillsDialog(_) => Slash {
            name: "skills",
            message: "Skills dialog dismissed",
        },
        M::PluginDialog(_) => Slash {
            name: "plugin",
            message: "Plugin dialog dismissed",
        },
        M::AgentsDialog(_) => Slash {
            name: "agents",
            message: "Agents dialog dismissed",
        },
        M::PermissionsEditor(_) => Slash {
            name: "permissions",
            message: "Permissions dialog dismissed",
        },
        M::McpServerSelect(_) => Slash {
            name: "mcp",
            message: "MCP dialog dismissed",
        },
        M::MemoryDialog(_) => Slash {
            name: "memory",
            message: "Cancelled memory editing",
        },
        M::Settings(_) => Slash {
            name: "status",
            message: "Status dialog dismissed",
        },
        M::DiffView(_) => Slash {
            name: "diff",
            message: "Diff dialog dismissed",
        },
        M::Export(_) => Slash {
            name: "export",
            message: "Export dialog dismissed",
        },
        M::SessionBrowser(_) => Slash {
            name: "resume",
            message: "Session browser dismissed",
        },
        M::CopyPicker(_) => Slash {
            name: "copy",
            message: "Copy cancelled",
        },
        M::QuickOpen(_) => System {
            message: "Quick open dismissed",
        },
        M::GlobalSearch(_) => System {
            message: "Search dismissed",
        },
        // Prompt / viewer / system modals own their own decline UX; Rewind
        // runs a dedicated multi-phase cancel (`modal_pane::rewind_cancel`).
        M::Error(_)
        | M::Transcript(_)
        | M::Rewind(_)
        | M::Doctor(_)
        | M::WorktreeExit(_)
        | M::Bridge(_)
        | M::InvalidConfig(_)
        | M::IdleReturn(_)
        | M::Trust(_)
        | M::BypassPermissions(_)
        | M::TaskDetail(_)
        | M::BackgroundTasks(_)
        | M::TeamRoster(_)
        | M::PluginHint(_)
        // `/add-dir` overlay runs its own Cancel (close + "Did not add…"
        // result) inside `add_directory::intercept`, so the generic Esc route
        // never reaches it.
        | M::AddDirectory(_)
        | M::Feedback(_) => return None,
    })
}

/// Close the active modal and surface its dismiss feedback. Shared by both Esc
/// routes: `TuiCommand::Cancel` and `TuiCommand::Deny` — the theme picker and
/// settings reuse the Settings keybinding context, whose Esc resolves to `Deny`
/// (`interaction::deny`), so the close logic must live in one place reachable
/// from both. Restores the theme picker's live preview before closing.
pub(crate) async fn close_modal_with_feedback(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    // Theme picker: Esc cancels the live preview by restoring the theme that was
    // active when the picker opened. Read `original_setting` before the take.
    if let Some(ModalState::ThemePicker(p)) = state.ui.modal.as_ref() {
        let original = p.original_setting.clone();
        if let Err(err) = state.ui.apply_theme_setting(original) {
            tracing::warn!(
                error = %err,
                "theme picker: failed to restore original theme on cancel"
            );
        }
    }
    // Plugin-hint Esc dismissal is treated as "no": record show-once
    // so the prompt never reappears.
    if let Some(ModalState::PluginHint(ph)) = state.ui.modal.as_ref() {
        coco_plugins::mark_hint_plugin_shown(&ph.plugin_id);
    }
    // Capture the dismiss feedback before the modal is taken, emit after close.
    let dismiss = state.ui.modal.as_ref().and_then(picker_dismiss);
    state.ui.dismiss_modal();
    match dismiss {
        Some(PickerDismiss::Slash { name, message }) => {
            let messages = coco_messages::build_slash_command_messages(
                name, /*args*/ "", message, /*is_sensitive*/ false,
            );
            let _ = command_tx
                .send(UserCommand::PushSlashResult { messages })
                .await;
        }
        Some(PickerDismiss::System { message }) => {
            let _ = command_tx
                .send(UserCommand::PushSystemMessage {
                    kind: SystemPushKind::Informational {
                        level: coco_messages::SystemMessageLevel::Info,
                        title: String::new(),
                        message: message.to_string(),
                    },
                })
                .await;
        }
        None => {}
    }
}

pub(crate) fn filter(state: &mut AppState, c: char) {
    match state.ui.modal.as_mut() {
        Some(ModalState::ModelPicker(m)) => {
            m.filter.push(c);
            m.selected = 0;
        }
        Some(ModalState::SessionBrowser(s)) => {
            s.filter.push(c);
            s.selected = 0;
        }
        Some(ModalState::GlobalSearch(g)) => {
            g.query.push(c);
            g.selected = 0;
        }
        Some(ModalState::QuickOpen(q)) => {
            q.filter.push(c);
            q.selected = 0;
        }
        _ => {}
    }
}

pub(crate) fn filter_backspace(state: &mut AppState) {
    match state.ui.modal.as_mut() {
        Some(ModalState::ModelPicker(m)) => {
            m.filter.pop();
            m.selected = 0;
        }
        Some(ModalState::SessionBrowser(s)) => {
            s.filter.pop();
            s.selected = 0;
        }
        Some(ModalState::GlobalSearch(g)) => {
            g.query.pop();
            g.selected = 0;
        }
        Some(ModalState::QuickOpen(q)) => {
            q.filter.pop();
            q.selected = 0;
        }
        _ => {}
    }
}

pub(crate) fn nav(state: &mut AppState, delta: i32) {
    let mut theme_preview: Option<crate::theme::ThemeSetting> = None;
    match state.ui.modal.as_mut() {
        Some(ModalState::ThemePicker(p)) => {
            let count = p.choices.len() as i32;
            let prev = p.selected;
            p.selected = (p.selected + delta).clamp(0, (count - 1).max(0));
            if p.selected != prev {
                theme_preview = p
                    .choices
                    .get(p.selected as usize)
                    .map(|c| c.setting.clone());
            }
        }
        Some(ModalState::ModelPicker(m)) => {
            let count = model_picker::filtered_models(m).len() as i32;
            m.selected = (m.selected + delta).clamp(0, (count - 1).max(0));
            m.effort = model_picker::filtered_models(m)
                .get(m.selected as usize)
                .and_then(|e| e.default_effort);
        }
        Some(ModalState::SessionBrowser(s)) => {
            let count = filtered_sessions(s).len() as i32;
            s.selected = (s.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::GlobalSearch(g)) => {
            let count = g.results.len() as i32;
            g.selected = (g.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::QuickOpen(q)) => {
            let count = q.files.len() as i32;
            q.selected = (q.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::Export(e)) => {
            let count = e.formats.len() as i32;
            e.selected = (e.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::Feedback(f)) => {
            let count = f.options.len() as i32;
            f.selected = (f.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::PluginHint(ph)) => {
            let count = crate::state::PluginHintState::OPTION_COUNT;
            ph.selected = (ph.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::Rewind(r)) => {
            update_rewind::handle_rewind_nav(r, delta);
        }
        Some(ModalState::DiffView(d)) => {
            d.scroll = (d.scroll + delta * constants::SCROLL_LINE_STEP).max(0);
        }
        Some(ModalState::TaskDetail(t)) => {
            t.scroll = (t.scroll + delta * constants::SCROLL_LINE_STEP).max(0);
        }
        Some(ModalState::Settings(s)) => {
            let count = settings::item_count(s) as i32;
            s.selected = (s.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::MemoryDialog(m)) => {
            let count = m.entries.len() as i32;
            m.selected = (m.selected + delta).clamp(0, (count - 1).max(0));
        }
        Some(ModalState::TeamRoster(r)) => {
            let count = r.members.len() as i32;
            let next = (r.selected as i32 + delta).clamp(0, (count - 1).max(0));
            r.selected = next as usize;
        }
        Some(ModalState::CopyPicker(cp)) => {
            if delta < 0 {
                for _ in 0..delta.unsigned_abs() {
                    cp.move_up();
                }
            } else {
                for _ in 0..delta as u32 {
                    cp.move_down();
                }
            }
        }
        Some(
            ModalState::Help
            | ModalState::Doctor(_)
            | ModalState::Bridge(_)
            | ModalState::InvalidConfig(_),
        ) => {
            state.ui.help_scroll =
                (state.ui.help_scroll + delta * constants::SCROLL_LINE_STEP).max(0);
        }
        _ => {}
    }
    if let Some(setting) = theme_preview {
        let _ = state.ui.preview_theme_setting(setting);
    }
}

pub(crate) async fn route_confirm(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    let Some(modal) = state.ui.take_modal() else {
        return false;
    };
    match modal {
        ModalState::ModelPicker(m) => {
            model_picker::confirm(state, m, command_tx).await;
        }
        ModalState::TeamRoster(_) => {
            state.ui.finish_taken_modal();
        }
        ModalState::Rewind(mut r) => {
            let rewound_turn = r.selected + 1;
            match update_rewind::handle_rewind_confirm(&mut r) {
                update_rewind::ConfirmOutcome::Dispatch {
                    message_id,
                    restore,
                } => {
                    r.phase = crate::state::rewind::RewindPhase::Confirming;
                    state.ui.restore_modal(ModalState::Rewind(r));
                    let _ = command_tx
                        .send(UserCommand::Rewind {
                            message_id,
                            mode: crate::command::RewindMode::Explicit {
                                restore_type: restore,
                                rewound_turn,
                            },
                        })
                        .await;
                }
                update_rewind::ConfirmOutcome::Phase => {
                    state.ui.restore_modal(ModalState::Rewind(r));
                }
                update_rewind::ConfirmOutcome::RequestDiffStats { message_id } => {
                    state.ui.restore_modal(ModalState::Rewind(r));
                    let _ = command_tx
                        .send(UserCommand::RequestDiffStats { message_id })
                        .await;
                }
                update_rewind::ConfirmOutcome::Dismiss => {
                    state.ui.finish_taken_modal();
                }
            }
        }
        ModalState::SessionBrowser(s) => {
            if let Some(session) = filtered_sessions(&s).get(s.selected as usize)
                && let Ok(name) = crate::state::SlashCommandName::new("resume")
            {
                let _ = command_tx
                    .send(UserCommand::ExecuteSlashCommand {
                        name,
                        args: session.id.clone(),
                    })
                    .await;
            }
            state.ui.finish_taken_modal();
        }
        ModalState::Export(e) => {
            if let Some(fmt) = e.formats.get(e.selected as usize)
                && let Ok(name) = crate::state::SlashCommandName::new("export")
            {
                // Dispatch the bare format keyword; the CLI runner expands it
                // to a timestamped default filename and writes the file.
                let args = match fmt {
                    ExportFormat::Markdown => "markdown",
                    ExportFormat::Json => "json",
                    ExportFormat::Text => "text",
                };
                let _ = command_tx
                    .send(UserCommand::ExecuteSlashCommand {
                        name,
                        args: args.to_string(),
                    })
                    .await;
            }
            state.ui.finish_taken_modal();
        }
        ModalState::ThemePicker(p) => {
            if let Some(choice) = p.choices.get(p.selected.max(0) as usize).cloned() {
                match state.ui.apply_theme_setting(choice.setting.clone()) {
                    Ok(()) => match crate::theme::save_theme_setting(&choice.setting) {
                        Ok(_path) => {
                            let messages = coco_messages::build_slash_command_messages(
                                "theme",
                                /*args*/ "",
                                &format!("Theme set to {}", choice.label),
                                /*is_sensitive*/ false,
                            );
                            let _ = command_tx
                                .send(crate::command::UserCommand::PushSlashResult { messages })
                                .await;
                        }
                        Err(err) => state.ui.add_toast(crate::state::ui::Toast::error(format!(
                            "Failed to save theme: {err}"
                        ))),
                    },
                    Err(err) => state.ui.add_toast(crate::state::ui::Toast::error(format!(
                        "Failed to apply theme: {err}"
                    ))),
                }
            }
            state.ui.finish_taken_modal();
        }
        ModalState::Settings(s) => {
            settings::confirm(state, s);
        }
        ModalState::MemoryDialog(m) => {
            if let Some(entry) = m.entries.get(m.selected as usize).cloned() {
                if entry.row_kind.is_file() {
                    let _ = command_tx
                        .send(UserCommand::OpenMemoryFile { path: entry.path })
                        .await;
                    state.ui.finish_taken_modal();
                } else {
                    state.ui.add_toast(Toast::warning(
                        t!("toast.memory_row_not_editable").to_string(),
                    ));
                    state.ui.restore_modal(ModalState::MemoryDialog(m));
                }
            } else {
                state.ui.finish_taken_modal();
            }
        }
        ModalState::Transcript(t) => {
            state.ui.restore_modal(ModalState::Transcript(t));
        }
        ModalState::CopyPicker(cp) => {
            if let Some(message) = crate::copy::confirm_picker_selection(state, cp) {
                crate::copy::enqueue_copy_output(message, command_tx);
            }
            state.ui.finish_taken_modal();
        }
        _ => {
            state.ui.finish_taken_modal();
        }
    }
    true
}

pub(crate) async fn request_diff_stats_if_rewind(
    state: &AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    if let Some(ModalState::Rewind(r)) = state.ui.modal.as_ref()
        && let Some(msg) = r.messages.get(r.selected as usize)
        && !msg.is_current_prompt
    {
        let _ = command_tx
            .send(UserCommand::RequestDiffStats {
                message_id: msg.message_id.to_string(),
            })
            .await;
    }
}

pub(crate) fn rewind_cancel(state: &mut AppState) -> bool {
    if let Some(ModalState::Rewind(r)) = state.ui.modal.as_mut()
        && !update_rewind::handle_rewind_cancel(r)
    {
        return false;
    }
    true
}

fn filtered_sessions(s: &SessionBrowserState) -> Vec<&SessionOption> {
    let filter_lower = s.filter.to_lowercase();
    s.sessions
        .iter()
        .filter(|sess| filter_lower.is_empty() || sess.label.to_lowercase().contains(&filter_lower))
        .collect()
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
