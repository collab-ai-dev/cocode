use super::*;
use crate::command::UserCommand;
use crate::state::AgentsDialogState;
use crate::state::AgentsDialogTab;
use crate::state::AppState;
use crate::state::CreateWizardStep;
use crate::state::LibraryRow;
use crate::state::ModalState;
use crate::state::WizardSource;
use coco_types::AgentSource;
use pretty_assertions::assert_eq;

fn library_with_create() -> Vec<LibraryRow> {
    vec![
        LibraryRow::CreateNew,
        LibraryRow::SourceHeader {
            label: "User agents".into(),
        },
        LibraryRow::Agent {
            name: "my-agent".into(),
            description: Some("test".into()),
            source: AgentSource::UserSettings,
            color: None,
            is_builtin: false,
            is_overridden: false,
            running_count: 0,
            source_path: Some(std::path::PathBuf::from("/tmp/my-agent.md")),
        },
    ]
}

fn dialog_state(library: Vec<LibraryRow>) -> AgentsDialogState {
    let mut state = AgentsDialogState::new(library);
    state.selected_tab = AgentsDialogTab::Library;
    state
}

#[tokio::test]
async fn submit_on_editable_agent_dispatches_open_editor() {
    let mut state = AppState::new();
    state.ui.show_modal(ModalState::AgentsDialog(
        dialog_state(library_with_create()),
    ));
    // Move cursor to the my-agent row (index 2; header at 1 skipped
    // by nav_library).
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut() {
        d.library_cursor = 2;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    on_library_submit(&mut state, &tx).await;
    let received = rx.try_recv().unwrap();
    assert!(matches!(
        received,
        UserCommand::OpenAgentEditor { path }
            if path == std::path::Path::new("/tmp/my-agent.md")
    ));
}

#[tokio::test]
async fn submit_on_create_new_opens_wizard_no_command() {
    let mut state = AppState::new();
    state.ui.show_modal(ModalState::AgentsDialog(
        dialog_state(library_with_create()),
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    on_library_submit(&mut state, &tx).await;
    assert!(
        rx.try_recv().is_err(),
        "no command dispatched for CreateNew; wizard opens locally"
    );
    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => panic!("dialog should still be open"),
    };
    assert!(dialog.is_in_wizard());
    assert_eq!(
        dialog.wizard.as_ref().map(|w| w.step),
        Some(CreateWizardStep::Name),
    );
}

#[tokio::test]
async fn delete_arms_confirm_then_dispatches_on_yes() {
    let mut state = AppState::new();
    state.ui.show_modal(ModalState::AgentsDialog(
        dialog_state(library_with_create()),
    ));
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut() {
        d.library_cursor = 2;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    // 'd' arms the confirmation but must NOT dispatch the destructive command.
    intercept(&mut state, &TuiCommand::InsertChar('d'), &tx).await;
    assert!(
        rx.try_recv().is_err(),
        "delete must not dispatch before confirmation"
    );
    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::AgentsDialog(d)) if d.pending_delete.is_some()),
        "pending_delete must be armed after 'd'"
    );
    // 'y' confirms → dispatch + clear.
    intercept(&mut state, &TuiCommand::InsertChar('y'), &tx).await;
    let received = rx.try_recv().unwrap();
    assert!(matches!(
        received,
        UserCommand::DeleteAgentFile { path }
            if path == std::path::Path::new("/tmp/my-agent.md")
    ));
    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::AgentsDialog(d)) if d.pending_delete.is_none()),
        "pending_delete cleared after confirm"
    );
}

#[tokio::test]
async fn delete_confirm_cancel_does_not_dispatch() {
    let mut state = AppState::new();
    state.ui.show_modal(ModalState::AgentsDialog(
        dialog_state(library_with_create()),
    ));
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut() {
        d.library_cursor = 2;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    intercept(&mut state, &TuiCommand::InsertChar('d'), &tx).await;
    intercept(&mut state, &TuiCommand::InsertChar('n'), &tx).await;
    assert!(
        rx.try_recv().is_err(),
        "cancel ('n') must not dispatch the delete"
    );
    assert!(
        matches!(state.ui.modal.as_ref(), Some(ModalState::AgentsDialog(d)) if d.pending_delete.is_none()),
        "pending_delete cleared after cancel"
    );
}

#[test]
fn cycle_tab_left_and_right() {
    let mut state = AppState::new();
    state
        .ui
        .show_modal(ModalState::AgentsDialog(AgentsDialogState::new(vec![
            LibraryRow::CreateNew,
        ])));
    cycle_tab(&mut state, 1);
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_ref() {
        assert_eq!(d.selected_tab, AgentsDialogTab::Library);
    } else {
        panic!("modal must be AgentsDialog");
    }
}

// ── Wizard flow ────────────────────────────────────────────────────

fn open_wizard(state: &mut AppState) {
    state.ui.show_modal(ModalState::AgentsDialog(
        dialog_state(library_with_create()),
    ));
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut() {
        d.open_wizard();
    }
}

/// Walk through Name → Description → Source. Returns the wizard
/// poised on the Confirm step with the assertions verified.
async fn advance_to_confirm(
    state: &mut AppState,
    tx: &tokio::sync::mpsc::Sender<UserCommand>,
    name: &str,
    description: &str,
    target: WizardSource,
) {
    for c in name.chars() {
        wizard_input_char(state, c);
    }
    wizard_advance(state, tx).await;
    let step = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.step);
    assert_eq!(step, Some(CreateWizardStep::Description));

    for c in description.chars() {
        wizard_input_char(state, c);
    }
    wizard_advance(state, tx).await;
    let step = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.step);
    assert_eq!(step, Some(CreateWizardStep::Source));

    // Cycle the source until it matches the requested target. Two
    // options, deterministic from User → Project on a single nav.
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut()
        && let Some(w) = d.wizard.as_mut()
    {
        while w.source != target {
            w.source = w.source.cycled(1);
        }
    }
    wizard_advance(state, tx).await;
    let step = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.step);
    assert_eq!(step, Some(CreateWizardStep::Confirm));
}

#[tokio::test]
async fn wizard_full_path_emits_create_agent() {
    // Use isolated tempdirs so the test never touches the real
    // workspace tree (no `.coco/agents/` pollution).
    let cwd_tmp = tempfile::tempdir().expect("cwd tempdir");
    let cfg_tmp = tempfile::tempdir().expect("config_home tempdir");

    let mut state = AppState::new();
    open_wizard(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);

    advance_to_confirm(
        &mut state,
        &tx,
        "my-agent",
        "Handles XYZ.",
        WizardSource::User,
    )
    .await;

    // Finalize against the testable pure-paths helper.
    wizard_finalize_with(&mut state, &tx, cwd_tmp.path(), cfg_tmp.path()).await;

    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => panic!("dialog must remain mounted"),
    };
    assert!(
        !dialog.is_in_wizard(),
        "successful dispatch must close the wizard"
    );

    match rx.try_recv().expect("CreateAgent should be queued") {
        UserCommand::CreateAgent {
            name,
            description,
            source,
        } => {
            assert_eq!(name, "my-agent");
            assert_eq!(description, "Handles XYZ.");
            assert_eq!(source, AgentSource::UserSettings);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn wizard_confirm_dispatch_blocked_when_file_exists() {
    use std::io::Write;
    // Seed a collision inside an isolated tempdir so the pre-flight
    // catches it without ever touching the workspace tree.
    let cwd_tmp = tempfile::tempdir().expect("cwd tempdir");
    let cfg_tmp = tempfile::tempdir().expect("config_home tempdir");
    let user_dir = cfg_tmp.path().join("agents");
    std::fs::create_dir_all(&user_dir).expect("create user agents dir");
    let collision_path = user_dir.join("my-agent.md");
    {
        let mut f = std::fs::File::create(&collision_path).expect("create collision file");
        writeln!(f, "---\nname: my-agent\n---\n").ok();
    }

    let mut state = AppState::new();
    open_wizard(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    advance_to_confirm(
        &mut state,
        &tx,
        "my-agent",
        "collision test",
        WizardSource::User,
    )
    .await;
    wizard_finalize_with(&mut state, &tx, cwd_tmp.path(), cfg_tmp.path()).await;

    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => panic!("dialog must remain mounted"),
    };
    assert!(
        dialog.is_in_wizard(),
        "collision must keep the wizard open so the user can rename"
    );
    let err = dialog
        .wizard
        .as_ref()
        .and_then(|w| w.error.as_ref())
        .expect("pre-flight error expected");
    match err {
        crate::state::WizardError::AlreadyExists { path } => {
            assert_eq!(path, &collision_path);
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert!(
        rx.try_recv().is_err(),
        "CreateAgent must not dispatch on collision"
    );
    // tempdirs Drop cleans up the seeded file automatically.
}

#[tokio::test]
async fn wizard_invalid_name_surfaces_error_and_stays() {
    let mut state = AppState::new();
    open_wizard(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);

    // Leading digit — invalid.
    for c in "3plan".chars() {
        wizard_input_char(&mut state, c);
    }
    wizard_advance(&mut state, &tx).await;

    let wizard = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .expect("wizard still active");
    assert_eq!(wizard.step, CreateWizardStep::Name);
    assert!(wizard.error.is_some());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn wizard_esc_walks_back_then_cancels() {
    let mut state = AppState::new();
    open_wizard(&mut state);
    let (tx, _rx) = tokio::sync::mpsc::channel::<UserCommand>(4);

    // Step Name → Description.
    for c in "alpha".chars() {
        wizard_input_char(&mut state, c);
    }
    wizard_advance(&mut state, &tx).await;
    // Esc back to Name.
    wizard_back_or_cancel(&mut state);
    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => panic!(),
    };
    assert_eq!(
        dialog.wizard.as_ref().map(|w| w.step),
        Some(CreateWizardStep::Name)
    );

    // Esc on Name closes the wizard but keeps the dialog open.
    wizard_back_or_cancel(&mut state);
    let dialog = match state.ui.modal.as_ref() {
        Some(ModalState::AgentsDialog(d)) => d,
        _ => panic!("dialog should stay mounted"),
    };
    assert!(!dialog.is_in_wizard());
}

#[test]
fn wizard_source_step_swallows_text_input() {
    let mut state = AppState::new();
    open_wizard(&mut state);
    // Force into Source step.
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut()
        && let Some(w) = d.wizard.as_mut()
    {
        w.step = CreateWizardStep::Source;
    }
    let before = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| (w.name.clone(), w.description.clone()));
    let changed = wizard_input_char(&mut state, 'x');
    // Source-step input must NOT mutate the text fields AND must
    // not request a redraw (W-14).
    assert!(!changed, "Source step input must report no-op");
    let after = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| (w.name.clone(), w.description.clone()));
    assert_eq!(before, after);
}

#[tokio::test]
async fn wizard_esc_from_confirm_returns_to_source() {
    let mut state = AppState::new();
    open_wizard(&mut state);
    let (tx, _rx) = tokio::sync::mpsc::channel::<UserCommand>(4);

    // Drive through to Confirm.
    let name = format!("coco_test_{}", std::process::id());
    advance_to_confirm(&mut state, &tx, &name, "test", WizardSource::User).await;

    // Esc from Confirm → Source.
    wizard_back_or_cancel(&mut state);
    let step = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.step);
    assert_eq!(step, Some(CreateWizardStep::Source));

    // Esc again → Description.
    wizard_back_or_cancel(&mut state);
    let step = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.step);
    assert_eq!(step, Some(CreateWizardStep::Description));
}

#[test]
fn wizard_description_rejects_control_chars() {
    // Tab / Ctrl-* shouldn't sneak past the wizard input filter and
    // corrupt the YAML body. The text input only accepts printable
    // characters; control chars are dropped silently.
    let mut state = AppState::new();
    open_wizard(&mut state);
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut()
        && let Some(w) = d.wizard.as_mut()
    {
        w.step = CreateWizardStep::Description;
    }
    assert!(wizard_input_char(&mut state, 'a'));
    // Tab is a control char in this codepath — must be rejected.
    assert!(!wizard_input_char(&mut state, '\t'));
    assert!(wizard_input_char(&mut state, 'b'));
    let desc = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.description.text.clone());
    assert_eq!(desc.as_deref(), Some("ab"));
}

#[test]
fn wizard_source_nav_cycles() {
    let mut state = AppState::new();
    open_wizard(&mut state);
    if let Some(ModalState::AgentsDialog(d)) = state.ui.modal.as_mut()
        && let Some(w) = d.wizard.as_mut()
    {
        w.step = CreateWizardStep::Source;
        assert_eq!(w.source, WizardSource::User);
    }
    wizard_source_nav(&mut state, 1);
    let src = state
        .ui
        .modal
        .as_ref()
        .and_then(|m| match m {
            ModalState::AgentsDialog(d) => d.wizard.as_ref(),
            _ => None,
        })
        .map(|w| w.source);
    assert_eq!(src, Some(WizardSource::Project));
}
