use super::*;
use crate::events::TuiCommand;
use crate::state::ModalState;
use crate::state::SkillOverrideState;
use crate::state::SkillsDialogState;
use coco_types::SkillsDialogEntry;
use coco_types::SkillsDialogPayload;

fn dialog_with(entries: Vec<SkillsDialogEntry>) -> SkillsDialogState {
    SkillsDialogState::from_wire(SkillsDialogPayload {
        entries,
        bytes_per_token: 4,
    })
}

fn entry(name: &str, source: coco_types::SkillsDialogSource) -> SkillsDialogEntry {
    SkillsDialogEntry {
        name: name.to_string(),
        source,
        description: String::new(),
        plugin_name: None,
        frontmatter_bytes: 100,
        current_local: None,
        baseline: coco_types::SkillOverrideState::On,
        lock: None,
    }
}

#[tokio::test]
async fn space_cycles_focused_in_select_mode() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::InsertChar(' '), &tx).await;
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert_eq!(dialog.rows[0].pending, SkillOverrideState::NameOnly);
}

#[tokio::test]
async fn slash_enters_filter_mode_in_select() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::InsertChar('/'), &tx).await;
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert!(dialog.filter_focused);
    assert!(dialog.filter_query.is_empty());
}

#[tokio::test]
async fn t_in_select_mode_toggles_sort_in_filter_mode_appends() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::InsertChar('t'), &tx).await;
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert!(dialog.sort_by_tokens);
    // Now activate filter mode and verify `t` is literal.
    let dialog_mut = match state.ui.modal.as_mut().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    dialog_mut.filter_focused = true;
    let _ = intercept(&mut state, &TuiCommand::InsertChar('t'), &tx).await;
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert_eq!(dialog.filter_query, "t");
    // sort_by_tokens stays toggled — filter-mode `t` is literal,
    // not a re-toggle.
    assert!(dialog.sort_by_tokens);
}

#[tokio::test]
async fn slash_is_stripped_inside_filter_mode_too() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    // Activate filter mode with one slash, then try to type another
    // slash — should be stripped, not appended.
    let _ = intercept(&mut state, &TuiCommand::InsertChar('/'), &tx).await;
    let _ = intercept(&mut state, &TuiCommand::InsertChar('/'), &tx).await;
    let _ = intercept(&mut state, &TuiCommand::InsertChar('a'), &tx).await;
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert_eq!(dialog.filter_query, "a");
}

#[tokio::test]
async fn enter_in_select_emits_write_command_and_dismisses_modal() {
    let mut state = AppState::new();
    let mut e = entry("foo", coco_types::SkillsDialogSource::User);
    e.baseline = coco_types::SkillOverrideState::On;
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![e])));
    {
        let d = match state.ui.modal.as_mut().unwrap() {
            ModalState::SkillsDialog(d) => d,
            _ => panic!(),
        };
        d.rows[0].pending = SkillOverrideState::Off;
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::SubmitInput, &tx).await;

    assert!(state.ui.modal.is_none(), "save dismisses the modal");
    // `total_edits` should be stashed on UiState, NOT shipped on the wire.
    assert_eq!(state.ui.pending_skills_save_edits, Some(1));
    let sent = rx.try_recv().expect("should send a command");
    match sent {
        UserCommand::WriteSkillOverrides { patch } => {
            assert_eq!(
                patch.get("skill_overrides").and_then(|v| v.get("foo")),
                Some(&serde_json::json!("off"))
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn enter_with_no_diff_skips_round_trip_and_renders_local_toast() {
    // User opened /skills, didn't toggle anything (or toggled and
    // reverted to baseline). Enter should dismiss the dialog and
    // surface the "No changes" toast locally — no UserCommand emitted.
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let _locale = crate::i18n::locale_test_guard("en");
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::SubmitInput, &tx).await;
    assert!(state.ui.modal.is_none());
    assert!(
        rx.try_recv().is_err(),
        "no diff ⇒ no WriteSkillOverrides round-trip"
    );
    // Toast got pushed locally.
    assert!(
        state
            .ui
            .toasts
            .iter()
            .any(|t| t.message.contains("No changes"))
    );
}

#[tokio::test]
async fn esc_in_filter_focus_clears_query_dialog_stays_open() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    {
        let d = match state.ui.modal.as_mut().unwrap() {
            ModalState::SkillsDialog(d) => d,
            _ => panic!(),
        };
        d.filter_focused = true;
        d.filter_query.push_str("abc");
    }
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::Cancel, &tx).await;
    assert!(
        state.ui.modal.is_some(),
        "Esc in filter only clears, not closes"
    );
    let dialog = match state.ui.modal.as_ref().unwrap() {
        ModalState::SkillsDialog(d) => d,
        _ => panic!(),
    };
    assert!(dialog.filter_query.is_empty());
    assert!(!dialog.filter_focused);
}

#[tokio::test]
async fn esc_in_select_mode_closes_dialog() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::SkillsDialog(dialog_with(vec![entry(
            "foo",
            coco_types::SkillsDialogSource::User,
        )])));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let _ = intercept(&mut state, &TuiCommand::Cancel, &tx).await;
    assert!(
        state.ui.modal.is_none(),
        "Esc in select mode dismisses the dialog"
    );
}
