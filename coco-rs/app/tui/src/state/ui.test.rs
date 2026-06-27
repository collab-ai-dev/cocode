//! Unit tests for [`PromptMode`] prefix detection and [`InputState`] mode
//! derivation.

use super::InputState;
use super::PromptMode;
use super::Toast;
use super::UiState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::SlashCommandName;
use pretty_assertions::assert_eq;

#[test]
fn prompt_mode_from_empty_is_normal() {
    assert_eq!(PromptMode::from_text(""), PromptMode::Normal);
}

#[test]
fn prompt_mode_bang_prefix_is_bash() {
    assert_eq!(PromptMode::from_text("!ls -la"), PromptMode::Bash);
    assert_eq!(PromptMode::from_text("!"), PromptMode::Bash);
    assert_eq!(PromptMode::from_text("! echo hi"), PromptMode::Bash);
}

#[test]
fn prompt_mode_leading_space_kills_prefix() {
    // Leading whitespace defeats the prefix match.
    assert_eq!(PromptMode::from_text(" !ls"), PromptMode::Normal);
    assert_eq!(PromptMode::from_text("\t#x"), PromptMode::Normal);
}

#[test]
fn prompt_mode_text_passthrough_for_other_chars() {
    assert_eq!(PromptMode::from_text("hello"), PromptMode::Normal);
    assert_eq!(PromptMode::from_text("#note this"), PromptMode::Normal);
    assert_eq!(PromptMode::from_text("# note"), PromptMode::Normal);
    assert_eq!(PromptMode::from_text("/help"), PromptMode::Normal);
    assert_eq!(PromptMode::from_text("@file.rs"), PromptMode::Normal);
}

#[test]
fn strip_prefix_normal_passes_text_through() {
    assert_eq!(PromptMode::Normal.strip_prefix("hello"), "hello");
    assert_eq!(PromptMode::Normal.strip_prefix(""), "");
}

#[test]
fn strip_prefix_bash_drops_bang_and_one_space() {
    assert_eq!(PromptMode::Bash.strip_prefix("!ls"), "ls");
    assert_eq!(PromptMode::Bash.strip_prefix("! ls"), "ls");
    // Multiple leading spaces: only one consumed (strip one optional space after `!`).
    assert_eq!(PromptMode::Bash.strip_prefix("!  ls"), " ls");
    assert_eq!(PromptMode::Bash.strip_prefix("!"), "");
}

#[test]
fn input_state_prompt_mode_tracks_text() {
    let mut state = InputState::new();
    assert_eq!(state.prompt_mode(), PromptMode::Normal);

    state.textarea.insert_str("!");
    assert_eq!(state.prompt_mode(), PromptMode::Bash);

    state.textarea.insert_str("ls");
    assert_eq!(state.prompt_mode(), PromptMode::Bash);

    // Delete the prefix (move home, forward-delete) — back to Normal.
    state.textarea.set_cursor(0);
    state.textarea.delete_forward(1);
    assert_eq!(state.prompt_mode(), PromptMode::Normal);
    assert_eq!(state.text(), "ls");
}

#[test]
fn input_state_prompt_mode_hash_is_normal_then_swap_to_bang() {
    let mut state = InputState::new();
    state.textarea.insert_str("#");
    assert_eq!(state.prompt_mode(), PromptMode::Normal);

    state.textarea.set_cursor(0);
    state.textarea.delete_forward(1);
    state.textarea.set_cursor(0);
    state.textarea.insert_str("!");
    assert_eq!(state.prompt_mode(), PromptMode::Bash);
}

#[test]
fn title_i18n_keys_match_yaml_layout() {
    // The render layer looks these up via `t!(...)` so they must
    // exist in locales/*.yaml. Asserting the literal here catches
    // refactors that rename keys without updating both files.
    assert_eq!(PromptMode::Normal.title_i18n_key(), "input.title");
    assert_eq!(PromptMode::Bash.title_i18n_key(), "input.title_bash_mode");
}

fn permission_prompt() -> PermissionPromptState {
    PermissionPromptState {
        request_id: "permission-1".to_string(),
        tool_name: "Bash".to_string(),
        description: "Run command".to_string(),
        detail: PermissionDetail::Generic {
            input_preview: "echo hi".to_string(),
        },
        risk_level: None,
        show_always_allow: false,
        classifier_checking: false,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("echo hi".to_string()),
        original_input: None,
        cwd: None,
        permission_suggestions: vec![],
        worker_badge: None,
        explanation_visible: false,
        explanation: crate::state::ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

#[test]
fn set_permission_prompt_routes_to_interaction_prompt() {
    let mut ui = UiState::new();

    ui.push_prompt(PanePromptState::Permission(permission_prompt()));

    assert!(matches!(
        ui.interaction.active_prompt,
        Some(PanePromptState::Permission(_))
    ));
    assert!(ui.modal.is_none());
}

#[test]
fn show_help_routes_to_modal_state() {
    let mut ui = UiState::new();

    ui.show_modal(ModalState::Help);

    assert!(matches!(ui.modal, Some(ModalState::Help)));
    assert!(ui.interaction.active_prompt.is_none());
}

#[test]
fn toast_does_not_create_prompt_or_modal() {
    let mut ui = UiState::new();

    ui.add_toast(Toast::info("saved"));

    assert!(ui.has_toasts());
    assert!(ui.modal.is_none());
    assert!(ui.interaction.active_prompt.is_none());
}

#[test]
fn finish_taken_permission_prompt_clears_prompt_state() {
    let mut ui = UiState::new();

    ui.push_prompt(PanePromptState::Permission(permission_prompt()));
    let prompt = ui.take_prompt();
    assert!(matches!(prompt, Some(PanePromptState::Permission(_))));

    ui.finish_taken_prompt();

    assert!(ui.interaction.active_prompt.is_none());
    assert!(ui.modal.is_none());
    assert!(!ui.has_blocking_interaction());
}

#[test]
fn restore_taken_surface_reinstalls_exact_surface_without_queueing() {
    let mut ui = UiState::new();

    ui.push_prompt(PanePromptState::Permission(permission_prompt()));
    let Some(mut prompt) = ui.take_prompt() else {
        panic!("expected active prompt");
    };
    if let PanePromptState::Permission(prompt) = &mut prompt {
        prompt.classifier_checking = true;
    }

    ui.restore_prompt(prompt);

    assert!(matches!(
        ui.interaction.active_prompt,
        Some(PanePromptState::Permission(PermissionPromptState {
            classifier_checking: true,
            ..
        }))
    ));
    assert_eq!(ui.interaction.prompt_queue.len(), 0);
}

#[test]
fn finish_taken_modal_clears_modal_state() {
    let mut ui = UiState::new();

    ui.show_modal(ModalState::Error("failure".to_string()));
    let modal = ui.take_modal();
    assert!(matches!(modal, Some(ModalState::Error(_))));

    ui.finish_taken_modal();

    assert!(ui.modal.is_none());
    assert!(ui.interaction.active_prompt.is_none());
    assert!(!ui.has_blocking_interaction());
}

#[test]
fn slash_command_name_validation_rejects_non_names() {
    assert_eq!(
        SlashCommandName::new("help")
            .expect("valid slash command")
            .as_str(),
        "help"
    );
    assert!(SlashCommandName::new("").is_err());
    assert!(SlashCommandName::new("/help").is_err());
    assert!(SlashCommandName::new("skill/run").is_err());
    assert!(SlashCommandName::new("help now").is_err());
    assert!(SlashCommandName::new("help\n").is_err());
}

#[test]
fn advance_display_holds_partial_trailing_line_until_newline() {
    let mut s = super::StreamingState::new();
    s.append_text("line one\npartial");

    // One tick reveals the complete line; the partial trailing line stays
    // hidden so its (re-wrapping) height cannot jitter the live region.
    assert!(s.advance_display());
    assert_eq!(s.visible_content(), "line one\n");

    // Further ticks reveal nothing while only the partial line is pending.
    assert!(!s.advance_display());
    assert_eq!(s.visible_content(), "line one\n");

    // Once the partial line gains its newline it becomes revealable.
    s.append_text(" more\nrest");
    assert!(s.advance_display());
    assert_eq!(s.visible_content(), "line one\npartial more\n");

    // reveal_all (finalize) shows the final partial line.
    s.reveal_all();
    assert_eq!(s.visible_content(), "line one\npartial more\nrest");
}
