use super::*;
use crate::state::AppState;
use crate::state::DeleteTarget;
use crate::state::ModalState;
use crate::state::PermEditorError;
use crate::state::PermissionsEditorTab;
use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::PermissionsEditorDir;
use coco_types::PermissionsEditorPayload;
use coco_types::PermissionsEditorRule;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

fn payload() -> PermissionsEditorPayload {
    PermissionsEditorPayload {
        rules: vec![PermissionsEditorRule {
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::LocalSettings,
            tool_pattern: "Read".into(),
            rule_content: None,
        }],
        directories: vec![PermissionsEditorDir {
            path: "/tmp/extra".into(),
            source: PermissionRuleSource::ProjectSettings,
        }],
        cwd: "/work".into(),
        managed_only: false,
    }
}

fn state_with_editor() -> AppState {
    let mut state = AppState::new();
    let mut editor = PermissionsEditorState::from_payload(payload());
    editor.selected_tab = PermissionsEditorTab::Allow;
    state.ui.show_modal(ModalState::PermissionsEditor(editor));
    state
}

fn state_with_recent_editor() -> AppState {
    let mut state = AppState::new();
    state.ui.record_recent_denial(
        "Bash".into(),
        "Bash".into(),
        "destructive filesystem operation".into(),
    );
    let mut editor = PermissionsEditorState::from_payload(payload());
    editor.set_recent_denials(state.ui.recent_denials_snapshot());
    state.ui.show_modal(ModalState::PermissionsEditor(editor));
    state
}

fn editor(state: &AppState) -> &PermissionsEditorState {
    match state.ui.modal.as_ref() {
        Some(ModalState::PermissionsEditor(e)) => e,
        _ => panic!("editor modal should be open"),
    }
}

#[test]
fn submit_on_add_sentinel_opens_form() {
    let mut state = state_with_editor();
    // Cursor defaults to row 0 = the Add sentinel.
    assert!(on_submit(&mut state));
    assert!(editor(&state).add_form.is_some());
}

#[tokio::test]
async fn approved_recent_denial_is_removed_and_granted_on_close() {
    let mut state = state_with_recent_editor();
    let (tx, mut rx) = mpsc::channel(4);

    assert!(on_submit(&mut state), "Enter toggles approved");
    close_editor(&mut state, &tx).await;

    assert!(
        state.ui.recent_denials.is_empty(),
        "approved denial should be removed from the session ring"
    );
    assert!(state.ui.modal.is_none(), "editor should close");
    let cmd = rx.try_recv().expect("grant system message should dispatch");
    match cmd {
        UserCommand::PushSystemMessage {
            kind: SystemPushKind::PermissionRetry { tool_name, message },
        } => {
            assert_eq!(tool_name, "Bash");
            assert_eq!(
                message,
                "Permission granted for: Bash. You may now retry this command if you would like."
            );
        }
        other => panic!("expected permission retry grant, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_recent_denial_is_removed_and_requery_requested_on_close() {
    let mut state = state_with_recent_editor();
    let (tx, mut rx) = mpsc::channel(4);

    assert!(toggle_recent_retry(&mut state), "r toggles retry");
    close_editor(&mut state, &tx).await;

    assert!(
        state.ui.recent_denials.is_empty(),
        "retry denial should be removed from the session ring"
    );
    assert!(state.ui.modal.is_none(), "editor should close");
    let cmd = rx.try_recv().expect("retry command should dispatch");
    match cmd {
        UserCommand::RetryPermissionDenied { tool_name, message } => {
            assert_eq!(tool_name, "Bash");
            assert_eq!(
                message,
                "Permission granted for: Bash. You may now retry this command if you would like."
            );
        }
        other => panic!("expected permission retry command, got {other:?}"),
    }
}

#[test]
fn submit_on_editable_rule_opens_delete_confirm() {
    let mut state = state_with_editor();
    // Move to the single Allow rule (row 1).
    nav(&mut state, 1);
    assert!(on_submit(&mut state));
    let confirm = editor(&state)
        .delete_confirm
        .as_ref()
        .expect("delete confirm should open");
    assert!(!confirm.yes, "defaults to the safe No");
    assert!(matches!(confirm.target, DeleteTarget::Rule(_)));
}

#[test]
fn route_paste_inserts_into_add_form_on_input_step() {
    let mut state = state_with_editor();
    on_submit(&mut state); // open the add form (starts on Input step)

    // Multi-line clipboard: newlines/tabs are stripped so the rule stays on
    // one physical line.
    assert!(route_paste(&mut state, "Bash(git\t*)\n"));

    let form = editor(&state).add_form.as_ref().unwrap();
    assert_eq!(form.input.text, "Bash(git*)");
    assert_eq!(form.error, None);
}

#[tokio::test]
async fn route_paste_ignored_off_input_step() {
    let mut state = state_with_editor();
    assert!(
        !route_paste(&mut state, "x"),
        "no add form open ⇒ paste must not be consumed"
    );

    // Advance to the destination step, where there is no text field.
    on_submit(&mut state);
    for c in "Bash(ls)".chars() {
        add_form_input_char(&mut state, c);
    }
    let (tx, _rx) = mpsc::channel(4);
    add_form_advance(&mut state, &tx).await;
    assert_eq!(
        editor(&state).add_form.as_ref().map(|f| f.step),
        Some(AddStep::Destination)
    );
    assert!(
        !route_paste(&mut state, "leak"),
        "destination step has no text field ⇒ paste must not be consumed"
    );
}

#[tokio::test]
async fn add_rule_flow_emits_add_rules_update() {
    let mut state = state_with_editor();
    let (tx, mut rx) = mpsc::channel(4);

    // Open the add form.
    on_submit(&mut state);
    // Type a pattern.
    for c in "Bash(git *)".chars() {
        add_form_input_char(&mut state, c);
    }
    // Advance Input -> Destination.
    add_form_advance(&mut state, &tx).await;
    assert_eq!(
        editor(&state).add_form.as_ref().map(|f| f.step),
        Some(AddStep::Destination)
    );
    // Pick the Project destination (Local -> Project).
    add_form_dest_nav(&mut state, 1);
    // Commit.
    add_form_advance(&mut state, &tx).await;

    let cmd = rx.try_recv().expect("update should be dispatched");
    match cmd {
        UserCommand::ApplyPermissionUpdate {
            update: PermissionUpdate::AddRules { rules, destination },
        } => {
            assert_eq!(destination, PermissionUpdateDestination::ProjectSettings);
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].behavior, PermissionBehavior::Allow);
            assert_eq!(rules[0].value.tool_pattern, "Bash");
            assert_eq!(rules[0].value.rule_content.as_deref(), Some("git *"));
        }
        other => panic!("expected AddRules update, got {other:?}"),
    }
    // Form closes after commit.
    assert!(editor(&state).add_form.is_none());
}

#[tokio::test]
async fn empty_input_blocks_advance_with_error() {
    let mut state = state_with_editor();
    let (tx, _rx) = mpsc::channel(4);
    on_submit(&mut state); // open form
    add_form_advance(&mut state, &tx).await; // empty input
    let form = editor(&state).add_form.as_ref().unwrap();
    assert_eq!(form.step, AddStep::Input);
    assert_eq!(form.error, Some(PermEditorError::EmptyInput));
}

#[tokio::test]
async fn delete_confirm_yes_emits_remove_rules() {
    let mut state = state_with_editor();
    let (tx, mut rx) = mpsc::channel(4);
    nav(&mut state, 1); // onto the Allow rule
    on_submit(&mut state); // open delete confirm
    toggle_confirm(&mut state); // No -> Yes
    delete_confirm_submit(&mut state, &tx).await;

    let cmd = rx.try_recv().expect("remove update should dispatch");
    match cmd {
        UserCommand::ApplyPermissionUpdate {
            update: PermissionUpdate::RemoveRules { rules, destination },
        } => {
            assert_eq!(destination, PermissionUpdateDestination::LocalSettings);
            assert_eq!(rules[0].value.tool_pattern, "Read");
        }
        other => panic!("expected RemoveRules update, got {other:?}"),
    }
    assert!(editor(&state).delete_confirm.is_none());
}

#[tokio::test]
async fn delete_confirm_no_dispatches_nothing() {
    let mut state = state_with_editor();
    let (tx, mut rx) = mpsc::channel(4);
    nav(&mut state, 1);
    on_submit(&mut state); // open confirm, defaults to No
    delete_confirm_submit(&mut state, &tx).await;
    assert!(rx.try_recv().is_err(), "No must not dispatch a removal");
    assert!(editor(&state).delete_confirm.is_none());
}

#[test]
fn managed_only_blocks_add_and_delete() {
    let mut state = AppState::new();
    let mut p = payload();
    p.managed_only = true;
    state.ui.show_modal(ModalState::PermissionsEditor(
        PermissionsEditorState::from_payload(p),
    ));
    // Add sentinel: no form opens.
    assert!(!on_submit(&mut state));
    assert!(editor(&state).add_form.is_none());
    // Rule row: no delete confirm opens.
    nav(&mut state, 1);
    assert!(!on_submit(&mut state));
    assert!(editor(&state).delete_confirm.is_none());
}

#[tokio::test]
async fn workspace_add_emits_add_directories() {
    let mut state = state_with_editor();
    if let Some(ModalState::PermissionsEditor(e)) = state.ui.modal.as_mut() {
        e.selected_tab = PermissionsEditorTab::Workspace;
    }
    let (tx, mut rx) = mpsc::channel(4);
    on_submit(&mut state); // open dir-add form
    for c in "/srv/data".chars() {
        add_form_input_char(&mut state, c);
    }
    add_form_advance(&mut state, &tx).await; // -> Destination
    add_form_advance(&mut state, &tx).await; // commit (Local default)

    let cmd = rx.try_recv().expect("dir update should dispatch");
    match cmd {
        UserCommand::ApplyPermissionUpdate {
            update:
                PermissionUpdate::AddDirectories {
                    directories,
                    destination,
                },
        } => {
            assert_eq!(destination, PermissionUpdateDestination::LocalSettings);
            assert_eq!(directories, vec!["/srv/data".to_string()]);
        }
        other => panic!("expected AddDirectories update, got {other:?}"),
    }
}
