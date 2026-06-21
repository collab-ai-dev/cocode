//! Behavior tests for the `/add-dir` overlay key handler.

use super::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::WizardTextField;

#[test]
fn map_key_maps_text_input_essentials() {
    let plain = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
    assert!(matches!(
        map_key(plain(KeyCode::Char('a'))),
        Some(TuiCommand::InsertChar('a'))
    ));
    assert!(matches!(
        map_key(plain(KeyCode::Enter)),
        Some(TuiCommand::SubmitInput)
    ));
    assert!(matches!(
        map_key(plain(KeyCode::Esc)),
        Some(TuiCommand::Cancel)
    ));
    assert!(matches!(
        map_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(TuiCommand::Cancel)
    ));
    // Up/Down carry no meaning in a single-line field — left unmapped.
    assert!(map_key(plain(KeyCode::Up)).is_none());
}

fn open_state() -> AppState {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::AddDirectory(AddDirectoryState::new()));
    state
}

fn input_text(state: &AppState) -> Option<String> {
    match state.ui.modal.as_ref() {
        Some(ModalState::AddDirectory(s)) => Some(s.input.text.clone()),
        _ => None,
    }
}

fn error_text(state: &AppState) -> Option<String> {
    match state.ui.modal.as_ref() {
        Some(ModalState::AddDirectory(s)) => s.error.clone(),
        _ => None,
    }
}

#[tokio::test]
async fn typing_appends_and_clears_stale_error() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = open_state();
    if let Some(ModalState::AddDirectory(s)) = state.ui.modal.as_mut() {
        s.error = Some("stale".to_string());
    }
    let (tx, _rx) = mpsc::channel(8);

    for c in "/tmp".chars() {
        assert!(matches!(
            intercept(&mut state, &TuiCommand::InsertChar(c), &tx).await,
            Handled::Yes(true)
        ));
    }

    assert_eq!(input_text(&state).as_deref(), Some("/tmp"));
    assert_eq!(error_text(&state), None);
}

#[test]
fn route_paste_inserts_into_modal_and_strips_control_chars() {
    let mut state = open_state();
    if let Some(ModalState::AddDirectory(s)) = state.ui.modal.as_mut() {
        s.error = Some("stale".to_string());
    }

    // Multi-line clipboard: newlines/tabs are stripped so the path stays on
    // one physical line; the rest lands in the modal's own input field.
    assert!(route_paste(&mut state, "/tmp/a\n/tmp/b"));

    assert_eq!(input_text(&state).as_deref(), Some("/tmp/a/tmp/b"));
    assert_eq!(error_text(&state), None, "paste clears stale error");
}

#[test]
fn route_paste_ignored_when_modal_absent() {
    let mut state = AppState::new();
    assert!(
        !route_paste(&mut state, "/tmp"),
        "paste must not be consumed when the /add-dir modal is closed"
    );
}

#[tokio::test]
async fn submit_empty_sets_error_and_keeps_open() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = open_state();
    let (tx, mut rx) = mpsc::channel(8);

    let handled = intercept(&mut state, &TuiCommand::SubmitInput, &tx).await;
    assert!(matches!(handled, Handled::Yes(true)));

    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::AddDirectory(_))),
        "empty submit must keep the overlay open"
    );
    assert!(
        error_text(&state).is_some(),
        "empty submit must set an error"
    );
    assert!(rx.try_recv().is_err(), "empty submit dispatches nothing");
}

#[tokio::test]
async fn submit_valid_dir_dispatches_add_dir_and_closes() {
    let _locale = crate::i18n::locale_test_guard("en");
    let dir = std::env::temp_dir();
    let dir_str = dir.to_string_lossy().to_string();
    let mut state = open_state();
    if let Some(ModalState::AddDirectory(s)) = state.ui.modal.as_mut() {
        s.input = WizardTextField::seeded(&dir_str);
    }
    let (tx, mut rx) = mpsc::channel(8);

    intercept(&mut state, &TuiCommand::SubmitInput, &tx).await;

    assert!(state.ui.modal.is_none(), "valid submit closes the overlay");
    match rx.try_recv() {
        Ok(UserCommand::ExecuteSlashCommand { name, .. }) => {
            assert_eq!(name.as_str(), "add-dir");
        }
        _ => panic!("expected an ExecuteSlashCommand for add-dir"),
    }
}

#[tokio::test]
async fn cancel_closes_and_emits_transcript_result() {
    let _locale = crate::i18n::locale_test_guard("en");
    let mut state = open_state();
    let (tx, mut rx) = mpsc::channel(8);

    intercept(&mut state, &TuiCommand::Cancel, &tx).await;

    assert!(state.ui.modal.is_none(), "cancel closes the overlay");
    assert!(
        matches!(rx.try_recv(), Ok(UserCommand::PushSlashResult { .. })),
        "cancel emits a transcript result"
    );
}
