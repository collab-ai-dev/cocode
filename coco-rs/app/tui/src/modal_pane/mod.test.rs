use tokio::sync::mpsc;

use super::route_confirm;
use crate::command::UserCommand;
use crate::state::AppState;
use crate::state::MemoryDialogEntry;
use crate::state::MemoryDialogRowKind;
use crate::state::MemoryDialogScope;
use crate::state::MemoryDialogState;
use crate::state::ModalState;
use crate::state::ThemePickerState;
use crate::state::WorkflowPickerEntry;
use crate::state::WorkflowPickerState;

fn channel() -> (mpsc::Sender<UserCommand>, mpsc::Receiver<UserCommand>) {
    mpsc::channel(8)
}

fn queue_help_after_active(state: &mut AppState, active: ModalState) {
    state.ui.show_modal(active);
    state.ui.show_modal(ModalState::Help);
}

fn assert_help_active(state: &AppState) {
    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::Help)),
        "queued Help modal should become active, got {:?}",
        state.ui.modal
    );
}

#[tokio::test]
async fn theme_picker_confirm_advances_queued_modal() {
    let mut state = AppState::new();
    queue_help_after_active(
        &mut state,
        ModalState::ThemePicker(ThemePickerState {
            choices: Vec::new(),
            selected: 0,
            original_setting: crate::theme::ThemeSetting::default(),
        }),
    );
    let (tx, _rx) = channel();

    assert!(route_confirm(&mut state, &tx).await);

    assert_help_active(&state);
}

#[tokio::test]
async fn memory_dialog_file_confirm_advances_queued_modal() {
    let path = std::path::PathBuf::from("/tmp/coco-memory-test/CLAUDE.md");
    let mut state = AppState::new();
    queue_help_after_active(
        &mut state,
        ModalState::MemoryDialog(MemoryDialogState {
            entries: vec![MemoryDialogEntry {
                path: path.clone(),
                label: "Project memory".to_string(),
                scope: MemoryDialogScope::Project,
                row_kind: MemoryDialogRowKind::File {
                    exists: false,
                    read_only: false,
                },
            }],
            selected: 0,
        }),
    );
    let (tx, mut rx) = channel();

    assert!(route_confirm(&mut state, &tx).await);

    let UserCommand::OpenMemoryFile { path: sent_path } =
        rx.try_recv().expect("memory open command sent")
    else {
        panic!("expected OpenMemoryFile")
    };
    assert_eq!(sent_path, path);
    assert_help_active(&state);
}

#[tokio::test]
async fn workflow_picker_confirm_dispatches_selected_workflow() {
    let mut state = AppState::new();
    queue_help_after_active(
        &mut state,
        ModalState::WorkflowPicker(WorkflowPickerState {
            entries: vec![
                WorkflowPickerEntry {
                    name: "release".to_string(),
                    description: "Ship it".to_string(),
                    source_path: ".coco/workflows/release.ts".to_string(),
                },
                WorkflowPickerEntry {
                    name: "audit".to_string(),
                    description: "Inspect auth".to_string(),
                    source_path: ".claude/workflows/audit.js".to_string(),
                },
            ],
            filter: "auth".to_string(),
            selected: 0,
        }),
    );
    let (tx, mut rx) = channel();

    assert!(route_confirm(&mut state, &tx).await);

    let UserCommand::ExecuteSlashCommand { name, args } =
        rx.try_recv().expect("workflow slash command sent")
    else {
        panic!("expected ExecuteSlashCommand")
    };
    assert_eq!(name.as_str(), "workflow");
    assert_eq!(args, "audit");
    assert_help_active(&state);
}

#[tokio::test]
async fn rewind_dismiss_confirm_advances_queued_modal() {
    let mut state = AppState::new();
    let rewind = crate::update_rewind::build_rewind_state(&state);
    queue_help_after_active(&mut state, ModalState::Rewind(rewind));
    let (tx, mut rx) = channel();

    assert!(route_confirm(&mut state, &tx).await);

    assert!(rx.try_recv().is_err(), "dismiss should not dispatch rewind");
    assert_help_active(&state);
}
