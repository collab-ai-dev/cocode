//! End-to-end tests for the update dispatcher. Focused on cross-module
//! invariants that the per-submodule tests can't catch — in particular typed
//! slash-command routing from both `submit` and `QueueInput`, and the
//! clipboard-cache lifecycle around `ClearScreen`.

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::handle_command;
use crate::command::ShutdownReason;
use crate::command::UserCommand;
use crate::display_settings::DisplaySettingEditability;
use crate::display_settings::DisplaySettings;
use crate::events::TuiCommand;
use crate::state::ActiveSuggestions;
use crate::state::AppState;
use crate::state::MemoryDialogEntry;
use crate::state::MemoryDialogRowKind;
use crate::state::MemoryDialogScope;
use crate::state::MemoryDialogState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::QuickOpenState;
use crate::state::SessionBrowserState;
use crate::state::SessionOption;
use crate::state::SlashCommandInfo;
use crate::state::SlashCommandName;
use crate::state::SuggestionKind;
use crate::state::ui::ToastSeverity;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_types::CommandArgumentKind;
use coco_types::CommandTypeTag;

fn drained_channel() -> (mpsc::Sender<UserCommand>, mpsc::Receiver<UserCommand>) {
    mpsc::channel(8)
}

/// Next real `UserCommand` on the wire, transparently skipping the
/// `PersistPromptHistory` bookkeeping message that `submit` now emits beside
/// every `add_to_history`. Returns the same `Result` shape as `try_recv` so
/// callers keep their `Ok(..)` and error arms unchanged.
fn next_user_command(
    rx: &mut mpsc::Receiver<UserCommand>,
) -> Result<UserCommand, mpsc::error::TryRecvError> {
    loop {
        match rx.try_recv()? {
            UserCommand::PersistPromptHistory { .. } => continue,
            other => return Ok(other),
        }
    }
}

#[tokio::test]
async fn clear_screen_nulls_last_agent_markdown() {
    // Regression: without this, Ctrl+L (ClearScreen) would wipe the visible
    // transcript but leave the copy cache pointing at the now-invisible
    // response, so a subsequent /copy would surface content the user
    // just cleared.
    let mut state = AppState::new();
    state.session.last_agent_markdown = Some("yesterday's reply".to_string());
    crate::transcript::derive::test_helpers::push_assistant_text(
        &mut state.session,
        "yesterday's reply",
    );
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::ClearScreen, &tx).await;

    assert!(
        state.session.transcript.is_empty(),
        "ClearScreen should drop cells from the engine-derived transcript"
    );
    assert_eq!(
        state.session.last_agent_markdown, None,
        "ClearScreen must null the copy cache"
    );
}

#[test]
fn parse_slash_input_validates_command_names() {
    let (name, args) =
        super::edit::parse_slash_input("/ask hello there").expect("valid slash command");
    assert_eq!(name, "ask");
    assert_eq!(args, "hello there");

    assert_eq!(super::edit::parse_slash_input("plain text"), None);
    assert_eq!(super::edit::parse_slash_input("/"), None);
    assert_eq!(super::edit::parse_slash_input("//bad"), None);
    assert_eq!(
        SlashCommandName::new("bad name"),
        Err(crate::state::InvalidSlashCommandName)
    );
}

#[tokio::test]
async fn queue_input_of_slash_sends_queue_command() {
    // Slash input typed while the agent is streaming is queued in the
    // engine first. The CLI runner drains slash commands after the active
    // turn completes instead of executing them immediately from the TUI.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/compact foo");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::QueueInput, &tx).await;

    assert!(
        state.session.queued_commands.is_empty(),
        "no optimistic local push — display reconciles from the engine"
    );
    match rx.try_recv() {
        Ok(UserCommand::QueueCommand { prompt, images }) => {
            assert_eq!(prompt, "/compact foo");
            assert!(images.is_empty());
        }
        other => panic!("expected QueueCommand on the wire, got {other:?}"),
    }
    assert!(state.ui.input.is_empty(), "input should have been consumed");
}

#[tokio::test]
async fn submit_during_active_turn_steers_instead_of_preempting() {
    // Regression: a turn is in flight (tools / subagents running) but the token
    // stream has already ended, so `is_streaming()` is false. Pressing Enter
    // must still STEER (queue into the running turn) rather than starting a
    // fresh turn that hard-preempts the active one (`SystemPreempt`). The gate
    // is `turn_active()`, mirroring TS `queryGuard.isActive`.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("hello");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    // Turn active but NOT streaming — the post-stream tool/subagent phase.
    state
        .ui
        .ephemeral
        .start_turn("Working", std::time::Instant::now());
    assert!(!state.is_streaming());
    assert!(state.ui.ephemeral.turn_active());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::QueueCommand { prompt, .. }) => assert_eq!(prompt, "hello"),
        other => panic!("expected QueueCommand (steering), got {other:?}"),
    }
    assert!(state.ui.input.is_empty(), "input should have been consumed");
}

#[tokio::test]
async fn submit_slash_dispatches_typed_command_without_chat_echo() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/rewind last");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    assert!(
        state.session.transcript.is_empty(),
        "slash invocations are commands, not chat transcript entries"
    );
    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "rewind");
            assert_eq!(args, "last");
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_exit_command_shuts_down_via_double_press_path() {
    // `/exit` must funnel into the same shutdown mechanism as the Ctrl+C/Ctrl+D
    // double-press exit — NOT the registry handler (which only prints text).
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/exit");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    assert!(state.should_exit(), "/exit must quit the TUI");
    match next_user_command(&mut rx) {
        Ok(UserCommand::Shutdown { reason }) => {
            assert_eq!(reason, ShutdownReason::SlashCommand);
        }
        other => panic!("expected Shutdown(SlashCommand) on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_quit_alias_shuts_down() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/quit");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    assert!(state.should_exit(), "/quit must quit the TUI");
    assert!(
        matches!(
            next_user_command(&mut rx),
            Ok(UserCommand::Shutdown {
                reason: ShutdownReason::SlashCommand
            })
        ),
        "/quit alias must shut down like /exit"
    );
}

#[tokio::test]
async fn autocomplete_tab_completes_selected_slash_without_submitting() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "clear".into(),
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/cl");
    state.ui.input.textarea.set_cursor(3);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            label: "/clear".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: "cl".into(),
        trigger_pos: 0,
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "/clear ");
    assert!(
        rx.try_recv().is_err(),
        "Tab accepts the suggestion but does not submit it"
    );
}

#[tokio::test]
async fn autocomplete_enter_completes_and_submits_no_arg_slash_command() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "clear".into(),
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/cl");
    state.ui.input.textarea.set_cursor(3);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            label: "/clear".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: "cl".into(),
        trigger_pos: 0,
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert!(
        state.ui.input.is_empty(),
        "submitted command consumes input"
    );
    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "clear");
            assert!(args.is_empty());
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn autocomplete_enter_completes_arg_slash_command_without_submitting() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "add-dir".into(),
        argument_hint: Some("<path>".into()),
        argument_kind: CommandArgumentKind::DirectoryPath,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/add");
    state.ui.input.textarea.set_cursor(4);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            label: "/add-dir".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: "add".into(),
        trigger_pos: 0,
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "/add-dir ");
    assert_eq!(state.ui.input.inline_hint.as_deref(), Some(" <path>"));
    assert!(
        rx.try_recv().is_err(),
        "commands with argument hints should wait for user args"
    );
}

#[tokio::test]
async fn autocomplete_enter_submits_overlay_command_despite_optional_arg_hint() {
    // Regression: `/model` (a `local-jsx` overlay opener) advertises an optional
    // `[model]` hint but must still run on bare Enter so the picker opens —
    // accepting `/model` and pressing Enter must open the picker.
    // Previously the mere presence of an arg hint forced
    // completion-only mode, parking the input at `/model ` with no overlay.
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "model".into(),
        argument_hint: Some("[model]".into()),
        argument_kind: CommandArgumentKind::FreeText,
        kind: CommandTypeTag::LocalOverlay,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/mod");
    state.ui.input.textarea.set_cursor(4);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            label: "/model".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: "mod".into(),
        trigger_pos: 0,
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert!(
        state.ui.input.is_empty(),
        "submitted overlay command consumes input"
    );
    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "model");
            assert!(args.is_empty(), "no-arg overlay open carries empty args");
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn typing_after_arg_slash_completion_clears_inline_hint() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "add-dir".into(),
        argument_hint: Some("<path>".into()),
        argument_kind: CommandArgumentKind::DirectoryPath,
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/add");
    state.ui.input.textarea.set_cursor(4);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            label: "/add-dir".into(),
            description: None,
            metadata: None,
        }],
        selected: 0,
        query: "add".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;
    assert_eq!(state.ui.input.text(), "/add-dir ");
    assert_eq!(state.ui.input.inline_hint.as_deref(), Some(" <path>"));

    handle_command(&mut state, TuiCommand::InsertChar('s'), &tx).await;

    assert_eq!(state.ui.input.text(), "/add-dir s");
    assert!(state.ui.input.inline_hint.is_none());
}

#[tokio::test]
async fn autocomplete_tab_accepts_inline_ghost() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("then /cl");
    state.ui.input.textarea.set_cursor("then /cl".len());
    state.ui.input.set_inline_ghost(crate::state::InlineGhost {
        text: "ear".into(),
        insert_position: "then /cl".len(),
        replace_start: "then /cl".len(),
        replace_end: "then /cl".len(),
        replacement: "ear".into(),
        cursor_after_accept: "then /clear".len(),
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "then /clear");
    assert_eq!(state.ui.input.textarea.cursor(), "then /clear".len());
}

#[tokio::test]
async fn esc_dismisses_completion_until_token_changes() {
    let mut state = AppState::new();
    state.session.available_commands = vec![SlashCommandInfo {
        name: "clear".into(),
        ..SlashCommandInfo::default()
    }];
    state.ui.input.textarea.set_text("/cl");
    state.ui.input.textarea.set_cursor(3);
    crate::autocomplete::refresh_suggestions(&mut state);
    assert!(state.ui.completion.active.is_some());

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::Cancel, &tx).await;
    assert!(state.ui.completion.active.is_none());

    crate::autocomplete::refresh_suggestions(&mut state);
    assert!(state.ui.completion.active.is_none());

    state.ui.input.textarea.set_text("/cle");
    state.ui.input.textarea.set_cursor(4);
    crate::autocomplete::refresh_suggestions(&mut state);
    assert!(state.ui.completion.active.is_some());
}

#[tokio::test]
async fn prompt_suggestion_enter_submits_visible_suggestion() {
    let mut state = AppState::new();
    state.session.prompt_suggestions = vec!["Run the failing tests".into()];

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitPromptSuggestion, &tx).await;

    match next_user_command(&mut rx) {
        Ok(UserCommand::SubmitInput { content, .. }) => {
            assert_eq!(content, "Run the failing tests");
        }
        other => panic!("expected SubmitInput on the wire, got {other:?}"),
    }
    assert!(state.ui.input.is_empty());
    assert!(state.session.prompt_suggestions.is_empty());
}

#[tokio::test]
async fn autocomplete_tab_completes_common_file_prefix() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@s");
    state.ui.input.textarea.set_cursor(2);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![
            SuggestionItem {
                label: "src/lib.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
            SuggestionItem {
                label: "src/main.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
        ],
        selected: 0,
        query: "s".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@src/");
    assert_eq!(state.ui.input.textarea.cursor(), "@src/".len());
}

#[tokio::test]
async fn autocomplete_tab_does_not_invent_slash_for_partial_common_prefix() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@s");
    state.ui.input.textarea.set_cursor(2);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![
            SuggestionItem {
                label: "src/main.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
            SuggestionItem {
                label: "src/match.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
        ],
        selected: 0,
        query: "s".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@src/ma");
    assert_eq!(state.ui.input.textarea.cursor(), "@src/ma".len());
}

#[tokio::test]
async fn autocomplete_tab_completes_common_quoted_file_prefix() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/my pro");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![
            SuggestionItem {
                label: "/tmp/my project/src/lib.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
            SuggestionItem {
                label: "/tmp/my project/src/main.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
        ],
        selected: 0,
        query: "\"/tmp/my pro".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/my project/src/\"");
    assert_eq!(
        state.ui.input.textarea.cursor(),
        "@\"/tmp/my project/src/".len(),
        "cursor stays inside the quotes while drilling down"
    );
}

#[tokio::test]
async fn autocomplete_tab_keeps_directory_common_prefix_slash_inside_quotes() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/my pro");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![
            SuggestionItem {
                label: "/tmp/my project/api/lib".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
            SuggestionItem {
                label: "/tmp/my project/api/main".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
        ],
        selected: 0,
        query: "\"/tmp/my pro".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/my project/api/\"");
    assert_eq!(
        state.ui.input.textarea.cursor(),
        "@\"/tmp/my project/api/".len()
    );
}

#[tokio::test]
async fn autocomplete_enter_accepts_selected_path_not_common_prefix() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@s");
    state.ui.input.textarea.set_cursor(2);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![
            SuggestionItem {
                label: "src/lib.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
            SuggestionItem {
                label: "src/main.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            },
        ],
        selected: 1,
        query: "s".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@src/main.rs ");
}

#[tokio::test]
async fn autocomplete_accepts_at_directory_and_keeps_completion_active() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@s");
    state.ui.input.textarea.set_cursor(2);
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![SuggestionItem {
            label: "src/".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "s".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@src/");
    assert_eq!(state.ui.input.textarea.cursor(), "@src/".len());
    assert!(state.ui.completion.active.is_some());
}

#[tokio::test]
async fn autocomplete_submit_at_directory_finalizes_mention() {
    // Regression: Enter on a directory in the fuzzy `@` popup must finalize it
    // as a mention (trailing space, popup closed) — mirroring TS `handleEnter`
    // file-mode. It previously drilled in (kept the popup open, no space) like
    // Tab, leaving no way to confirm a directory selection.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@app");
    state.ui.input.textarea.set_cursor("@app".len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![SuggestionItem {
            label: "app/cli/".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "app".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@app/cli/ ");
    assert_eq!(state.ui.input.textarea.cursor(), "@app/cli/ ".len());
    assert!(
        state.ui.completion.active.is_none(),
        "finalizing a directory mention closes the popup"
    );
}

#[tokio::test]
async fn autocomplete_submit_at_directory_does_not_loop_on_identical_token() {
    // Reproduces the no-op loop: the typed token already equals the directory's
    // own path (the state the old drill-in logic got stuck in, because file
    // search returns the directory itself as the top hit for that query).
    // Enter must finalize + close, not re-insert the identical token.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@app/cli/");
    state.ui.input.textarea.set_cursor("@app/cli/".len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![SuggestionItem {
            label: "app/cli/".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "app/cli/".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@app/cli/ ");
    assert!(state.ui.completion.active.is_none());
}

#[tokio::test]
async fn autocomplete_submit_path_directory_drills_into_contents() {
    // Contrast with the fuzzy `@` case above: Enter in path-traversal mode
    // (`@./…`, `SuggestionKind::Path`) drills into the directory — appends `/`
    // with NO trailing space, so the still-live token re-opens the popup on the
    // directory's *contents*. Path search lists children (not the directory
    // itself), so this makes progress instead of looping.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@./sr");
    state.ui.input.textarea.set_cursor("@./sr".len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![SuggestionItem {
            label: "./src".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "./sr".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@./src/");
    assert!(
        state.ui.completion.active.is_some(),
        "drilling keeps the popup live on the new path token"
    );
}

#[tokio::test]
async fn final_quoted_file_accept_closes_quote_and_appends_space() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/my pro");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![SuggestionItem {
            label: "/tmp/my project/main.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }],
        selected: 0,
        query: "\"/tmp/my pro".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/my project/main.rs\" ");
    assert_eq!(
        state.ui.input.textarea.cursor(),
        state.ui.input.text().len()
    );
}

#[tokio::test]
async fn final_quoted_file_accept_replaces_drilldown_closing_quote() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/my project/\"");
    state
        .ui
        .input
        .textarea
        .set_cursor("@\"/tmp/my project/".len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![SuggestionItem {
            label: "/tmp/my project/main.rs".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }],
        selected: 0,
        query: "\"/tmp/my project/".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/my project/main.rs\" ");
}

#[tokio::test]
async fn quoted_path_accept_escapes_quote_and_backslash() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/a");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![SuggestionItem {
            label: "/tmp/a\"b\\c.txt".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path {
                is_directory: false,
            }),
        }],
        selected: 0,
        query: "\"/tmp/a".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/a\\\"b\\\\c.txt\" ");
}

#[tokio::test]
async fn directory_command_enter_submits_current_input() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/add-dir ./src");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Directory,
        items: vec![SuggestionItem {
            label: "./src".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "./src".into(),
        trigger_pos: "/add-dir ".len(),
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "add-dir");
            assert_eq!(args, "./src");
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_completion_inserts_session_id_and_submits() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/resume aut");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::CustomTitle,
        items: vec![SuggestionItem {
            label: "session-123".into(),
            description: Some("Auth refactor".into()),
            metadata: None,
        }],
        selected: 0,
        query: "aut".into(),
        trigger_pos: "/resume ".len(),
    });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteSubmit, &tx).await;

    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "resume");
            assert_eq!(args, "session-123");
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn mcp_resource_completion_preserves_server_name_on_insert() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@guide");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::At,
        items: vec![SuggestionItem {
            label: "Guide".into(),
            description: Some("Project guide".into()),
            metadata: Some(SuggestionMeta::McpResource {
                server: "docs-server".into(),
                uri: "file://guide".into(),
            }),
        }],
        selected: 0,
        query: "guide".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@docs-server:file://guide ");
}

#[tokio::test]
async fn directory_insertion_quotes_trailing_slash_inside_quotes() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("@\"/tmp/my pro");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::Path,
        items: vec![SuggestionItem {
            label: "/tmp/my project".into(),
            description: None,
            metadata: Some(SuggestionMeta::Path { is_directory: true }),
        }],
        selected: 0,
        query: "\"/tmp/my pro".into(),
        trigger_pos: 0,
    });

    let (tx, _rx) = drained_channel();
    handle_command(&mut state, TuiCommand::AutocompleteAccept, &tx).await;

    assert_eq!(state.ui.input.text(), "@\"/tmp/my project/\"");
}

#[tokio::test]
async fn submit_rewind_undo_alias_dispatches_typed_command() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("/undo");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    match next_user_command(&mut rx) {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "undo");
            assert!(args.is_empty());
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn session_browser_confirm_dispatches_resume_command() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SessionBrowser(SessionBrowserState {
            sessions: vec![SessionOption {
                id: "session-123".to_string(),
                label: "Auth refactor".to_string(),
                message_count: 8,
                created_at: "2026-05-23T00:00:00Z".to_string(),
            }],
            filter: String::new(),
            selected: 0,
        }));

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SurfaceConfirm, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::ExecuteSlashCommand { name, args }) => {
            assert_eq!(name, "resume");
            assert_eq!(args, "session-123");
        }
        other => panic!("expected ExecuteSlashCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn queue_input_of_plain_text_still_queues() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("write a haiku");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::QueueInput, &tx).await;

    // The TUI display is repopulated from the engine via the
    // `CommandQueued` notification round-trip (handled in
    // `server_notification_handler::protocol`), so the local store
    // stays empty until that event arrives. Asserting the channel
    // payload pins the wire-side contract.
    assert!(
        state.session.queued_commands.is_empty(),
        "no optimistic local push — display reconciles from the engine"
    );
    match rx.try_recv() {
        Ok(UserCommand::QueueCommand { prompt, images }) => {
            assert_eq!(prompt, "write a haiku");
            assert!(images.is_empty());
        }
        other => panic!("expected QueueCommand on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_input_while_streaming_without_interruptible_tool_queues() {
    let mut state = AppState::new();
    state.ui.streaming = Some(crate::state::StreamingState::default());
    // Streaming implies an active turn; the steering gate reads `turn_active`.
    state
        .ui
        .ephemeral
        .start_turn("Working", std::time::Instant::now());
    state.ui.input.textarea.set_text("next turn prompt");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::QueueCommand { prompt, images }) => {
            assert_eq!(prompt, "next turn prompt");
            assert!(images.is_empty());
        }
        other => panic!("expected QueueCommand on the wire, got {other:?}"),
    }
    assert!(
        rx.try_recv().is_err(),
        "non-interruptible streaming submit should only queue"
    );
}

#[tokio::test]
async fn submit_input_while_streaming_with_interruptible_tool_queues_then_interrupts() {
    let mut state = AppState::new();
    state.ui.streaming = Some(crate::state::StreamingState::default());
    // Streaming implies an active turn; the steering gate reads `turn_active`.
    state
        .ui
        .ephemeral
        .start_turn("Working", std::time::Instant::now());
    state.session.has_submit_interruptible_tool_in_progress = true;
    state.ui.input.textarea.set_text("follow-up prompt");
    state
        .ui
        .input
        .textarea
        .set_cursor(state.ui.input.text().len());

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::SubmitInput, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::QueueCommand { prompt, images }) => {
            assert_eq!(prompt, "follow-up prompt");
            assert!(images.is_empty());
        }
        other => panic!("expected QueueCommand first, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(UserCommand::Interrupt(coco_types::TurnAbortReason::SubmitInterrupt)) => {}
        other => panic!("expected submit interrupt second, got {other:?}"),
    }
}

#[tokio::test]
async fn up_on_empty_input_requests_edit_for_queued_commands() {
    let mut state = AppState::new();
    state
        .session
        .queued_commands
        .push_back(crate::state::QueuedCommandDisplay {
            id: "queued-1".to_string(),
            preview: "first queued".to_string(),
            editable: true,
        });

    let (tx, mut rx) = drained_channel();
    handle_command(&mut state, TuiCommand::CursorUp, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::EditQueuedCommands {
            current_input,
            current_cursor,
        }) => {
            assert_eq!(current_input, "");
            assert_eq!(current_cursor, 0);
        }
        other => panic!("expected EditQueuedCommands on the wire, got {other:?}"),
    }
}

#[test]
fn toggle_syntax_highlighting_does_not_mutate_when_higher_priority_setting_wins() {
    let mut state = AppState::new();
    state.ui.display_settings = DisplaySettings {
        syntax_highlighting: SyntaxHighlighting::Enabled,
        syntax_highlighting_editability: DisplaySettingEditability::OverriddenBy(
            coco_config::SettingSource::Project,
        ),
        show_thinking: false,
        copy_full_response: false,
        native_replay_cache: crate::transcript::render::HistoryReplayCachePolicy::default(),
        ..DisplaySettings::default()
    };

    crate::modal_pane::settings::toggle_syntax_highlighting(&mut state);

    assert_eq!(
        state.ui.display_settings.syntax_highlighting,
        SyntaxHighlighting::Enabled
    );
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Warning);
    assert!(
        state.ui.toasts[0].message.contains("project"),
        "unexpected toast: {}",
        state.ui.toasts[0].message
    );
}

#[tokio::test]
async fn idle_ctrl_c_arms_exit_hint_without_interrupting() {
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;

    assert_eq!(
        state.ui.pending_exit_hint(),
        Some(crate::state::ExitKey::CtrlC)
    );
    assert!(
        rx.try_recv().is_err(),
        "idle Ctrl+C should not send UserCommand::Interrupt"
    );
}

#[tokio::test]
async fn busy_ctrl_c_interrupts_without_exit_hint() {
    let mut state = AppState::new();
    state.session.set_busy(true);
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;

    assert_eq!(state.ui.pending_exit_hint(), None);
    // The interrupt visual is now an InterruptionMarker chat row
    // appended by `on_turn_interrupted` once `TurnInterrupted` arrives —
    // not a session-state boolean. This handler's only job here is to
    // forward the cancel signal.
    match rx.try_recv() {
        Ok(UserCommand::Interrupt(coco_types::TurnAbortReason::UserCancel)) => {}
        other => panic!("expected Interrupt on the wire, got {other:?}"),
    }
}

#[tokio::test]
async fn busy_escape_interrupts_without_clearing_input() {
    let mut state = AppState::new();
    state.session.set_busy(true);
    state.ui.input.textarea.set_text("draft");
    state.ui.input.textarea.set_cursor(5);
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    assert_eq!(state.ui.input.text(), "draft");
    match rx.try_recv() {
        Ok(UserCommand::Interrupt(coco_types::TurnAbortReason::UserCancel)) => {}
        other => panic!("expected UserCancel interrupt on Esc, got {other:?}"),
    }
}

#[tokio::test]
async fn escape_with_editable_queue_requests_batch_edit() {
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("draft");
    state.ui.input.textarea.set_cursor(2);
    state
        .session
        .queued_commands
        .push_back(crate::state::QueuedCommandDisplay {
            id: "queued-1".to_string(),
            preview: "first queued".to_string(),
            editable: true,
        });
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::EditQueuedCommands {
            current_input,
            current_cursor,
        }) => {
            assert_eq!(current_input, "draft");
            assert_eq!(current_cursor, 2);
        }
        other => panic!("expected EditQueuedCommands on Esc, got {other:?}"),
    }
}

#[tokio::test]
async fn up_ignores_non_editable_queue_and_uses_history() {
    let mut state = AppState::new();
    state.ui.input.add_to_history("prior".to_string());
    state
        .session
        .queued_commands
        .push_back(crate::state::QueuedCommandDisplay {
            id: "queued-1".to_string(),
            preview: "system queued".to_string(),
            editable: false,
        });
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CursorUp, &tx).await;

    assert_eq!(state.ui.input.text(), "prior");
    assert!(
        rx.try_recv().is_err(),
        "non-editable queue should not dispatch edit"
    );
}

#[tokio::test]
async fn up_on_lower_line_moves_cursor_instead_of_recalling_history() {
    let mut state = AppState::new();
    state.ui.input.add_to_history("prior".to_string());
    // Multi-line draft with the cursor parked on the second line.
    state.ui.input.textarea.set_text("line1\nline2");
    state.ui.input.textarea.set_cursor(8); // inside "line2"
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CursorUp, &tx).await;

    // Cursor moved within the draft; history was NOT recalled.
    assert_eq!(state.ui.input.text(), "line1\nline2");
    assert!(state.ui.input.history_index.is_none());
}

#[tokio::test]
async fn ctrl_r_reverse_search_previews_match_and_accepts() {
    let mut state = AppState::new();
    state.ui.input.add_to_history("git status".to_string());
    state.ui.input.add_to_history("cargo build".to_string());
    // history (newest-first): [cargo build, git status]
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::HistorySearchStart, &tx).await;
    assert!(state.ui.history_search.is_some());

    for c in "git".chars() {
        handle_command(&mut state, TuiCommand::HistorySearchInput(c), &tx).await;
    }
    // "git" matches the older entry; it previews in the composer.
    assert_eq!(state.ui.input.text(), "git status");
    assert_eq!(state.ui.history_search.as_ref().unwrap().matched, Some(1));

    handle_command(&mut state, TuiCommand::HistorySearchAccept, &tx).await;
    assert!(state.ui.history_search.is_none());
    assert_eq!(state.ui.input.text(), "git status");
}

#[tokio::test]
async fn ctrl_r_reverse_search_cancel_restores_draft() {
    let mut state = AppState::new();
    state.ui.input.add_to_history("git status".to_string());
    state.ui.input.textarea.set_text("draft text");
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::HistorySearchStart, &tx).await;
    handle_command(&mut state, TuiCommand::HistorySearchInput('g'), &tx).await;
    assert_eq!(state.ui.input.text(), "git status"); // previewing the match

    handle_command(&mut state, TuiCommand::HistorySearchCancel, &tx).await;
    assert!(state.ui.history_search.is_none());
    assert_eq!(state.ui.input.text(), "draft text"); // draft restored
}

#[tokio::test]
async fn escape_in_teammates_view_interrupts_focused_teammate_current_work() {
    let mut state = AppState::new();
    state.session.expanded_view = coco_types::ExpandedView::Teammates;
    state.session.focused_subagent_index = Some(0);
    state
        .session
        .subagents
        .push(crate::state::session::SubagentInstance {
            kind: crate::state::session::SubagentKind::Subagent,
            agent_id: "worker@team".into(),
            agent_type: "general".into(),
            description: "scan".into(),
            status: crate::state::session::SubagentStatus::Running,
            color: None,
            team_name: None,
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        });
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::InterruptAgentCurrentWork { agent_id }) => {
            assert_eq!(agent_id, "worker@team");
        }
        other => panic!("expected InterruptAgentCurrentWork, got {other:?}"),
    }
}

#[tokio::test]
async fn background_all_tasks_optimistically_flips_running_subagents() {
    // Ctrl+B emits no wire event for the foreground→background transition
    // (TS aligned). The TUI must flip its own Running BgAgent rows to
    // is_backgrounded flipped inline before forwarding the engine command,
    // otherwise the activity panel stays out of sync until the eventual
    // TaskCompleted.
    let mut state = AppState::new();
    state
        .session
        .subagents
        .push(crate::state::session::SubagentInstance {
            kind: crate::state::SubagentKind::Subagent,
            agent_id: "bg-1".into(),
            agent_type: "subagent".into(),
            description: "scan".into(),
            status: crate::state::SubagentStatus::Running,
            color: None,
            team_name: None,
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        });
    // A teammate row must NOT flip (only BgAgent subagents background).
    state
        .session
        .subagents
        .push(crate::state::session::SubagentInstance {
            kind: crate::state::SubagentKind::Teammate,
            agent_id: "tm@team".into(),
            agent_type: "researcher".into(),
            description: "research".into(),
            status: crate::state::SubagentStatus::Running,
            color: None,
            team_name: Some("team".into()),
            started_at_ms: None,
            last_tool_name: None,
            tool_count: 0,
            total_tokens: 0,
            is_backgrounded: false,
            recent_activities: Vec::new(),
            final_message: None,
            completed_at_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.0,
        });
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::BackgroundAllTasks, &tx).await;

    assert!(
        state.session.subagents[0].is_backgrounded,
        "BgAgent subagent must flip is_backgrounded inline"
    );
    assert_eq!(
        state.session.subagents[0].status,
        crate::state::SubagentStatus::Running,
        "Backgrounded BgAgent keeps Running status until TaskCompleted lands"
    );
    assert!(
        !state.session.subagents[1].is_backgrounded,
        "Teammate must NOT be backgrounded by Ctrl+B"
    );
    assert_eq!(
        state.session.subagents[1].status,
        crate::state::SubagentStatus::Running,
        "Teammate status unchanged"
    );
    match rx.try_recv() {
        Ok(UserCommand::BackgroundAllTasks) => {}
        other => panic!("expected BackgroundAllTasks forwarded to engine, got {other:?}"),
    }
}

#[tokio::test]
async fn shift_up_first_press_selects_first_agent_then_steps() {
    // Shift+↑ from the composer must park focus on the switcher AND land on
    // the FIRST agent (index 0), regardless of direction — the coco-rs switcher
    // has no synthetic `◯ main`/leader row to step off, so entry must select a
    // real agent immediately (one press = visible selection, not two).
    let mut state = AppState::new();
    let running = |id: &str| crate::state::session::SubagentInstance {
        kind: crate::state::SubagentKind::Subagent,
        agent_id: id.into(),
        agent_type: "Explore".into(),
        description: "scan".into(),
        status: crate::state::SubagentStatus::Running,
        color: None,
        team_name: None,
        started_at_ms: None,
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: 0.0,
    };
    state.session.subagents.push(running("a0"));
    state.session.subagents.push(running("a1"));
    state.session.subagents.push(running("a2"));
    let (tx, _rx) = drained_channel();

    // First Shift+↑ (delta = -1): focus parks on the switcher, selection = 0.
    handle_command(&mut state, TuiCommand::AgentSwitcherNav(-1), &tx).await;
    assert_eq!(state.ui.focus, crate::state::FocusTarget::AgentSwitcher);
    assert_eq!(state.ui.agent_switcher_selected, 0);

    // Once focused, Shift+↓ steps down to the next agent.
    handle_command(&mut state, TuiCommand::AgentSwitcherNav(1), &tx).await;
    assert_eq!(state.ui.agent_switcher_selected, 1);
}

#[tokio::test]
async fn background_all_tasks_no_op_when_nothing_running() {
    // No foreground tasks → Ctrl+B emits nothing and changes no state.
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::BackgroundAllTasks, &tx).await;

    assert!(rx.try_recv().is_err(), "no UserCommand should fire");
}

#[tokio::test]
async fn double_ctrl_c_shutdown_carries_reason() {
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;
    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::Shutdown { reason }) => {
            assert_eq!(reason, ShutdownReason::DoublePressCtrlC);
        }
        other => panic!("expected Shutdown(DoublePressCtrlC), got {other:?}"),
    }
    assert!(state.should_exit());
}

#[tokio::test]
async fn double_ctrl_d_shutdown_carries_reason() {
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::RequestExit, &tx).await;
    handle_command(&mut state, TuiCommand::RequestExit, &tx).await;

    match rx.try_recv() {
        Ok(UserCommand::Shutdown { reason }) => {
            assert_eq!(reason, ShutdownReason::DoublePressCtrlD);
        }
        other => panic!("expected Shutdown(DoublePressCtrlD), got {other:?}"),
    }
    assert!(state.should_exit());
}

#[tokio::test]
async fn clear_screen_preserves_active_surface() {
    // Defensive: ClearScreen should be safe to invoke with a surface open;
    // the surface is user-owned and unrelated to chat content.
    let mut state = AppState::new();
    state.ui.show_modal(crate::state::ModalState::Help);
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::ClearScreen, &tx).await;

    assert!(state.session.transcript.is_empty());
    // Surface is intentionally preserved — ClearScreen scopes to transcript.
    assert!(state.ui.has_active_surface());
}

// ── Plan mode state behavior ──

#[tokio::test]
async fn cycle_into_bypass_shows_confirmation_modal() {
    use coco_types::PermissionMode;

    let mut state = AppState::new();
    state.session.bypass_permissions_available = true;
    state.session.permission_mode = PermissionMode::Plan; // next = Bypass
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CyclePermissionMode, &tx).await;

    // Mode must NOT change until the user confirms.
    assert_eq!(state.session.permission_mode, PermissionMode::Plan);
    assert!(
        matches!(
            state.ui.modal.as_ref(),
            Some(ModalState::BypassPermissions(_))
        ),
        "BypassPermissionsState should be shown"
    );
    assert!(rx.try_recv().is_err(), "should not flip mode until approve");
}

#[tokio::test]
async fn approve_bypass_modal_flips_mode() {
    use crate::state::BypassPermissionsState;
    use coco_types::PermissionMode;

    let mut state = AppState::new();
    state.session.bypass_permissions_available = true;
    state
        .ui
        .show_modal(ModalState::BypassPermissions(BypassPermissionsState {
            current_mode: "Plan".into(),
        }));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Approve, &tx).await;

    assert_eq!(
        state.session.permission_mode,
        PermissionMode::BypassPermissions
    );
    assert!(!state.ui.has_active_surface());
    let cmd = rx.try_recv().expect("SetPermissionMode must be sent");
    assert!(
        matches!(
            cmd,
            UserCommand::SetPermissionMode {
                mode: PermissionMode::BypassPermissions
            }
        ),
        "got: {cmd:?}"
    );
}

#[tokio::test]
async fn deny_bypass_modal_keeps_mode() {
    use crate::state::BypassPermissionsState;
    use coco_types::PermissionMode;

    let mut state = AppState::new();
    state.session.bypass_permissions_available = true;
    state.session.permission_mode = PermissionMode::Plan;
    state
        .ui
        .show_modal(ModalState::BypassPermissions(BypassPermissionsState {
            current_mode: "Plan".into(),
        }));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Deny, &tx).await;

    assert_eq!(state.session.permission_mode, PermissionMode::Plan);
    assert!(!state.ui.has_active_surface());
    assert!(
        rx.try_recv().is_err(),
        "deny must not emit SetPermissionMode"
    );
}

#[tokio::test]
async fn cycle_into_auto_applies_immediately() {
    use coco_types::PermissionMode;

    // Only auto available — cycle Plan → Auto since bypass is gated off.
    // Auto is classifier-gated (not dangerous), so it switches directly
    // with no confirmation modal — only Bypass surfaces a dialog.
    let mut state = AppState::new();
    state.session.auto_mode_available = true;
    state.session.bypass_permissions_available = false;
    state.session.permission_mode = PermissionMode::Plan;
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CyclePermissionMode, &tx).await;

    assert_eq!(state.session.permission_mode, PermissionMode::Auto);
    assert!(
        !state.ui.has_active_surface(),
        "no confirmation modal for Auto"
    );
    assert!(state.ui.toasts.is_empty());
    let cmd = rx.try_recv().expect("SetPermissionMode must be sent");
    assert!(matches!(
        cmd,
        UserCommand::SetPermissionMode {
            mode: PermissionMode::Auto
        }
    ));
}

#[tokio::test]
async fn cycle_into_safe_mode_applies_immediately() {
    use coco_types::PermissionMode;

    let mut state = AppState::new();
    state.session.permission_mode = PermissionMode::Default;
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CyclePermissionMode, &tx).await;

    // Default → AcceptEdits with no confirmation state.
    assert_eq!(state.session.permission_mode, PermissionMode::AcceptEdits);
    assert!(!state.ui.has_active_surface());
    // No toast — the status bar reflects the active mode.
    assert!(state.ui.toasts.is_empty());
    let cmd = rx.try_recv().expect("SetPermissionMode must be sent");
    assert!(matches!(
        cmd,
        UserCommand::SetPermissionMode {
            mode: PermissionMode::AcceptEdits
        }
    ));
}

#[tokio::test]
async fn toggle_plan_mode_changes_mode() {
    use coco_types::PermissionMode;

    let mut state = AppState::new();
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::TogglePlanMode, &tx).await;
    assert_eq!(state.session.permission_mode, PermissionMode::Plan);
    // No toast — the status bar reflects the active mode.
    assert!(state.ui.toasts.is_empty());

    handle_command(&mut state, TuiCommand::TogglePlanMode, &tx).await;
    assert_eq!(state.session.permission_mode, PermissionMode::Default);
}

// ─────────────────── Behavior tests: Ctrl+C / ESC / Ctrl+E ──────────────────

#[tokio::test]
async fn ctrl_c_with_text_clears_input_and_saves_to_history() {
    // Ctrl+C with text present clears the input AND records it into history
    // so the user can recover it with Up. Per `update/exit.rs::on_interrupt`,
    // the exit hint is still pre-armed so a second Ctrl+C exits.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("draft text");
    state.ui.input.textarea.set_cursor(10);
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;

    assert!(state.ui.input.is_empty(), "input should have been cleared");
    assert!(
        state
            .ui
            .input
            .history
            .iter()
            .any(|h| h.text == "draft text"),
        "draft must be in history",
    );
    // Tracker armed so the next Ctrl+C goes through the Quit path.
    assert!(state.ui.ctrl_c_tracker.pending().is_some());
}

#[tokio::test]
async fn ctrl_c_idle_empty_arms_exit_then_quits() {
    // With empty input the first Ctrl+C only arms a hint; a second
    // within the window exits.
    let mut state = AppState::new();
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;
    assert!(state.ui.ctrl_c_tracker.pending().is_some());
    // Second press within the window should request shutdown.
    handle_command(&mut state, TuiCommand::Interrupt, &tx).await;
    // Quit drives `state.quit()`; we can't see process exit from a unit
    // test, but `running` flips to Done and no interrupt was sent.
    assert!(state.should_exit());
}

#[tokio::test]
async fn esc_with_text_first_press_shows_toast() {
    // When input is non-empty, single Esc shows a toast and arms the double-press.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("draft");
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    assert_eq!(
        state.ui.input.text(),
        "draft",
        "text must NOT clear on single Esc"
    );
    assert!(
        state.ui.toasts.iter().any(|t| t.message.contains("again")),
        "single Esc should toast 'Esc again to clear'",
    );
}

#[tokio::test]
async fn esc_double_press_clears_input_and_records_history() {
    // Double-press Esc within the window clears input + records history.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("draft");
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;
    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    assert!(state.ui.input.is_empty(), "double-Esc clears input");
    assert!(state.ui.input.history.iter().any(|h| h.text == "draft"));
}

#[tokio::test]
async fn history_recall_rehydrates_paste_pills() {
    // A draft containing a paste pill, cleared via double-Esc, must restore
    // its paste payload on Up-arrow recall — otherwise the recalled pill is
    // a dangling token that resolves to literal text at submit.
    let mut state = AppState::new();
    let pill = state
        .ui
        .paste_manager
        .add_text("the huge payload".to_string());
    state.ui.input.textarea.set_text(&format!("see {pill}"));
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;
    handle_command(&mut state, TuiCommand::Cancel, &tx).await;
    assert!(state.ui.input.is_empty(), "double-Esc clears input");
    assert!(
        state.ui.paste_manager.entries().is_empty(),
        "cleared draft must not leave stale paste entries on the manager",
    );

    handle_command(&mut state, TuiCommand::CursorUp, &tx).await;
    assert_eq!(state.ui.input.text(), format!("see {pill}"));
    let resolved = state
        .ui
        .paste_manager
        .resolve_structured(state.ui.input.text());
    assert_eq!(resolved.text, "see the huge payload");
}

#[tokio::test]
async fn esc_on_memory_dialog_records_transcript_result() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::MemoryDialog(MemoryDialogState {
            entries: vec![MemoryDialogEntry {
                path: std::path::PathBuf::from("/tmp/coco-memory-test/CLAUDE.md"),
                label: "Project memory".to_string(),
                scope: MemoryDialogScope::Project,
                row_kind: MemoryDialogRowKind::File {
                    exists: false,
                    read_only: false,
                },
            }],
            selected: 0,
        }));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    assert!(!state.ui.has_active_surface(), "memory dialog dismissed");
    // Dismiss emits a transcript system line (`❯ /memory` + `⎿ Cancelled memory editing`),
    // not a toast.
    match rx.try_recv() {
        Ok(UserCommand::PushSlashResult { messages }) => assert!(
            format!("{messages:?}").contains("Cancelled memory editing"),
            "expected dismiss text in {messages:?}"
        ),
        other => panic!("expected PushSlashResult, got {other:?}"),
    }
}

#[tokio::test]
async fn esc_on_theme_picker_emits_dismiss_slash_result() {
    // Esc on the theme picker emits "Theme picker dismissed".
    // The theme picker reuses the Settings keybinding context, whose Esc maps to
    // `Deny` (not `Cancel`) — the dismiss feedback must fire on that route too.
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::ThemePicker(crate::state::ThemePickerState {
            choices: state.ui.theme_state.choices.clone(),
            selected: 0,
            original_setting: crate::theme::ThemeSetting::default(),
        }));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Deny, &tx).await;

    assert!(!state.ui.has_active_surface(), "theme picker dismissed");
    match rx.try_recv() {
        Ok(UserCommand::PushSlashResult { messages }) => assert!(
            format!("{messages:?}").contains("Theme picker dismissed"),
            "expected dismiss text in {messages:?}"
        ),
        other => panic!("expected PushSlashResult, got {other:?}"),
    }
}

#[tokio::test]
async fn esc_on_quick_open_emits_system_dismiss_line() {
    // Keybinding-only overlay (no slash command) → a standalone system line,
    // not a `❯ /quick-open` echo.
    let mut state = AppState::new();
    state.ui.show_modal(ModalState::QuickOpen(QuickOpenState {
        filter: String::new(),
        files: vec!["a.rs".to_string()],
        selected: 0,
    }));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::Cancel, &tx).await;

    assert!(!state.ui.has_active_surface(), "quick open dismissed");
    match rx.try_recv() {
        Ok(UserCommand::PushSystemMessage { kind }) => assert!(
            format!("{kind:?}").contains("Quick open dismissed"),
            "expected dismiss text in {kind:?}"
        ),
        other => panic!("expected PushSystemMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn ctrl_e_moves_cursor_to_end_not_external_editor() {
    // Regression: bare Ctrl+E previously triggered OpenExternalEditor in
    // the legacy global cascade, shadowing readline's end-of-line. The
    // user now expects Ctrl+E → CursorEnd via `map_input_key`.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("hello");
    state.ui.input.textarea.set_cursor(0);
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CursorEnd, &tx).await;

    assert_eq!(state.ui.input.textarea.cursor(), 5);
}

#[tokio::test]
async fn open_external_editor_sends_prompt_editor_command() {
    let mut state = AppState::new();
    state.ui.input.set_text("draft prompt");
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::OpenExternalEditor, &tx).await;

    let UserCommand::OpenPromptEditor { initial_content } =
        rx.try_recv().expect("prompt editor command sent")
    else {
        panic!("expected OpenPromptEditor")
    };
    assert_eq!(initial_content, "draft prompt");
}

#[tokio::test]
async fn open_external_editor_while_busy_warns_without_dispatch() {
    let mut state = AppState::new();
    state.session.set_busy(true);
    state.ui.input.set_text("draft prompt");
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::OpenExternalEditor, &tx).await;

    assert!(
        rx.try_recv().is_err(),
        "busy prompt editor must not dispatch OpenPromptEditor"
    );
    assert!(state.ui.toasts.iter().any(|t| {
        t.severity == ToastSeverity::Warning
            && t.message.contains("unavailable while a turn is running")
    }));
}

#[tokio::test]
async fn open_external_editor_while_streaming_warns_without_dispatch() {
    let mut state = AppState::new();
    state.ui.streaming = Some(crate::state::StreamingState::default());
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::OpenExternalEditor, &tx).await;

    assert!(
        rx.try_recv().is_err(),
        "streaming prompt editor must not dispatch OpenPromptEditor"
    );
    assert!(state.ui.toasts.iter().any(|t| {
        t.severity == ToastSeverity::Warning
            && t.message.contains("unavailable while a turn is running")
    }));
}

#[tokio::test]
async fn open_plan_editor_sends_plan_editor_command() {
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::OpenPlanEditor, &tx).await;

    let UserCommand::OpenPlanEditor = rx.try_recv().expect("plan editor command sent") else {
        panic!("expected OpenPlanEditor")
    };
    assert!(state.ui.toasts.is_empty());
}

#[tokio::test]
async fn ctrl_a_moves_cursor_to_start_visually_correct_for_cjk() {
    // After typing CJK input, Ctrl+A must move cursor to byte 0. The
    // render-layer test (snapshot) covers the column-0 visual; here we
    // just confirm the state-level position.
    let mut state = AppState::new();
    state.ui.input.textarea.set_text("你好世界");
    state.ui.input.textarea.set_cursor(12); // end (4 chars × 3 bytes)
    let (tx, _rx) = drained_channel();

    handle_command(&mut state, TuiCommand::CursorHome, &tx).await;

    assert_eq!(state.ui.input.textarea.cursor(), 0);
}

fn toggle_test_prompt(request_id: &str) -> crate::state::PermissionPromptState {
    crate::state::PermissionPromptState {
        request_id: request_id.to_string(),
        tool_name: "Bash".to_string(),
        description: "Run?".to_string(),
        detail: crate::state::PermissionDetail::Generic {
            input_preview: "ls".to_string(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Text("ls".to_string()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

#[tokio::test]
async fn toggle_permission_explanation_opens_panel_and_requests_fetch() {
    let mut state = AppState::new();
    state
        .ui
        .push_prompt(PanePromptState::Permission(toggle_test_prompt("req-1")));
    let (tx, mut rx) = drained_channel();

    handle_command(&mut state, TuiCommand::TogglePermissionExplanation, &tx).await;

    // Panel opens, fetch kicked off (Loading), and a request is dispatched.
    match state.ui.interaction.active_prompt.as_ref() {
        Some(PanePromptState::Permission(p)) => {
            assert!(p.explanation_visible);
            assert!(matches!(
                p.explanation,
                crate::state::ExplainerFetch::Loading
            ));
        }
        _ => panic!("expected permission prompt"),
    }
    match rx.try_recv() {
        Ok(UserCommand::RequestPermissionExplanation { request_id, .. }) => {
            assert_eq!(request_id, "req-1");
        }
        other => panic!("expected RequestPermissionExplanation, got {other:?}"),
    }

    // Toggling again collapses the panel — no re-fetch (already Loading).
    handle_command(&mut state, TuiCommand::TogglePermissionExplanation, &tx).await;
    match state.ui.interaction.active_prompt.as_ref() {
        Some(PanePromptState::Permission(p)) => assert!(!p.explanation_visible),
        _ => panic!("expected permission prompt"),
    }
    assert!(rx.try_recv().is_err(), "must not re-request on re-toggle");
}

#[tokio::test]
async fn toggle_permission_explanation_noop_without_prompt() {
    let mut state = AppState::new();
    let (tx, mut rx) = drained_channel();
    // No active permission prompt → Ctrl+E does nothing and dispatches nothing.
    let changed = handle_command(&mut state, TuiCommand::TogglePermissionExplanation, &tx).await;
    assert!(!changed);
    assert!(rx.try_recv().is_err());
}
