use super::*;
use coco_types::AgentSource;
use pretty_assertions::assert_eq;

fn agent_row(name: &str, source: AgentSource) -> LibraryRow {
    LibraryRow::Agent {
        name: name.into(),
        description: None,
        source,
        color: None,
        is_builtin: matches!(source, AgentSource::BuiltIn),
        is_overridden: false,
        running_count: 0,
        source_path: None,
    }
}

fn header(label: &str) -> LibraryRow {
    LibraryRow::SourceHeader {
        label: label.into(),
    }
}

#[test]
fn default_focus_is_running_tab() {
    let state = AgentsDialogState::new(vec![LibraryRow::CreateNew]);
    assert_eq!(state.selected_tab, AgentsDialogTab::Running);
}

#[test]
fn tab_cycles_both_directions() {
    let mut tab = AgentsDialogTab::Running;
    tab = tab.cycled(1);
    assert_eq!(tab, AgentsDialogTab::Library);
    tab = tab.cycled(1);
    assert_eq!(tab, AgentsDialogTab::Running);
    tab = tab.cycled(-1);
    assert_eq!(tab, AgentsDialogTab::Library);
}

#[test]
fn nav_library_skips_headers() {
    let rows = vec![
        LibraryRow::CreateNew,
        header("User agents"),
        agent_row("alpha", AgentSource::UserSettings),
        header("Built-in agents"),
        agent_row("Plan", AgentSource::BuiltIn),
    ];
    let mut state = AgentsDialogState::new(rows);
    assert_eq!(state.library_cursor, 0); // CreateNew
    state.nav_library(1);
    assert_eq!(state.library_cursor, 2); // alpha (header skipped)
    state.nav_library(1);
    assert_eq!(state.library_cursor, 4); // Plan
    state.nav_library(1);
    assert_eq!(state.library_cursor, 0); // wraps to CreateNew
    state.nav_library(-1);
    assert_eq!(state.library_cursor, 4); // wraps back to Plan
}

#[test]
fn snap_clamps_when_list_shrinks() {
    let rows = vec![
        LibraryRow::CreateNew,
        agent_row("alpha", AgentSource::UserSettings),
        agent_row("beta", AgentSource::UserSettings),
    ];
    let mut state = AgentsDialogState::new(rows);
    state.library_cursor = 2; // beta
    state.library = vec![LibraryRow::CreateNew];
    state.snap_library_cursor();
    assert_eq!(state.library_cursor, 0);
}

#[test]
fn snap_walks_off_header_after_rebuild() {
    let mut state = AgentsDialogState::new(vec![
        LibraryRow::CreateNew,
        agent_row("alpha", AgentSource::UserSettings),
    ]);
    state.library_cursor = 1;
    // Rebuild library with the previous focus now on a header.
    state.library = vec![
        header("User agents"),
        agent_row("alpha", AgentSource::UserSettings),
    ];
    // Cursor at 1 IS a selectable row, but if the row at 0 was the
    // header (which it now is), focusing the header should snap to
    // the next selectable. Test the explicit header-focus case:
    state.library_cursor = 0;
    state.snap_library_cursor();
    assert_eq!(state.library_cursor, 1);
}

// ── Create wizard ──────────────────────────────────────────────────

#[test]
fn validate_name_accepts_kebab_and_pascal() {
    assert!(validate_agent_name("general-purpose").is_ok());
    assert!(validate_agent_name("Plan").is_ok());
    assert!(validate_agent_name("coco_guide").is_ok());
    assert!(validate_agent_name("Agent42").is_ok());
}

#[test]
fn validate_name_rejects_empty_or_whitespace() {
    assert_eq!(validate_agent_name(""), Err(WizardError::NameEmpty));
    assert_eq!(validate_agent_name("   "), Err(WizardError::NameEmpty));
}

#[test]
fn validate_name_rejects_leading_digit() {
    assert_eq!(validate_agent_name("3agent"), Err(WizardError::NameLead));
}

#[test]
fn validate_name_rejects_punctuation_and_spaces() {
    assert_eq!(validate_agent_name("my agent"), Err(WizardError::NameChars));
    assert_eq!(validate_agent_name("hello!"), Err(WizardError::NameChars));
    assert_eq!(
        validate_agent_name("path/segment"),
        Err(WizardError::NameChars)
    );
}

#[test]
fn wizard_source_cycles_user_and_project() {
    let mut src = WizardSource::User;
    src = src.cycled(1);
    assert_eq!(src, WizardSource::Project);
    src = src.cycled(1);
    assert_eq!(src, WizardSource::User);
    src = src.cycled(-1);
    assert_eq!(src, WizardSource::Project);
}

#[test]
fn wizard_source_maps_to_agent_source() {
    assert_eq!(
        WizardSource::User.as_agent_source(),
        AgentSource::UserSettings
    );
    assert_eq!(
        WizardSource::Project.as_agent_source(),
        AgentSource::ProjectSettings
    );
}

#[test]
fn open_wizard_then_close_round_trips() {
    let mut state = AgentsDialogState::new(vec![LibraryRow::CreateNew]);
    assert!(!state.is_in_wizard());
    state.open_wizard();
    assert!(state.is_in_wizard());
    assert_eq!(
        state.wizard.as_ref().map(|w| w.step),
        Some(CreateWizardStep::Name)
    );
    state.close_wizard();
    assert!(!state.is_in_wizard());
}

// ── WizardTextField cursor semantics ───────────────────────────────

#[test]
fn text_field_insert_advances_cursor() {
    let mut f = WizardTextField::new();
    f.insert_char('a');
    f.insert_char('b');
    f.insert_char('c');
    assert_eq!(f.text, "abc");
    assert_eq!(f.cursor, 3);
}

#[test]
fn text_field_insert_at_middle_cursor() {
    let mut f = WizardTextField::seeded("ac");
    f.move_left();
    assert_eq!(f.cursor, 1);
    f.insert_char('b');
    assert_eq!(f.text, "abc");
    assert_eq!(f.cursor, 2);
}

#[test]
fn text_field_delete_back_from_middle() {
    let mut f = WizardTextField::seeded("abcd");
    f.move_left();
    f.move_left();
    // cursor between 'b' and 'c'
    f.delete_back();
    assert_eq!(f.text, "acd");
    assert_eq!(f.cursor, 1);
}

#[test]
fn text_field_delete_forward_from_middle() {
    let mut f = WizardTextField::seeded("abcd");
    f.move_left();
    f.move_left();
    f.delete_forward();
    assert_eq!(f.text, "abd");
    assert_eq!(f.cursor, 2);
}

#[test]
fn text_field_home_end_clamp() {
    let mut f = WizardTextField::seeded("abc");
    f.move_home();
    assert_eq!(f.cursor, 0);
    f.move_left();
    assert_eq!(f.cursor, 0, "left at home should stay at 0");
    f.move_end();
    assert_eq!(f.cursor, 3);
    f.move_right();
    assert_eq!(f.cursor, 3, "right at end should stay at len");
}

#[test]
fn text_field_handles_utf8_grapheme_boundaries() {
    let mut f = WizardTextField::seeded("héllo");
    // cursor at end = 5 chars
    assert_eq!(f.cursor, 5);
    f.move_left();
    f.delete_back();
    // removed 'l' before 'o'
    assert_eq!(f.text, "hélo");
}

#[test]
fn split_at_cursor_renders_caret_position() {
    let mut f = WizardTextField::seeded("hello");
    f.move_home();
    let (before, after) = f.split_at_cursor();
    assert_eq!(before, "");
    assert_eq!(after, "hello");
    f.move_right();
    let (before, after) = f.split_at_cursor();
    assert_eq!(before, "h");
    assert_eq!(after, "ello");
}

// ── resolve_create_target pure helper ──────────────────────────────

#[test]
fn resolve_target_user_source_paths_under_config_home() {
    let tmp_cwd = tempfile::tempdir().unwrap();
    let tmp_cfg = tempfile::tempdir().unwrap();
    let target = resolve_create_target(
        AgentSource::UserSettings,
        "demo",
        tmp_cwd.path(),
        tmp_cfg.path(),
    )
    .expect("clean target");
    assert_eq!(target, tmp_cfg.path().join("agents").join("demo.md"));
}

#[test]
fn resolve_target_project_source_paths_under_cwd_coco() {
    let tmp_cwd = tempfile::tempdir().unwrap();
    let tmp_cfg = tempfile::tempdir().unwrap();
    let target = resolve_create_target(
        AgentSource::ProjectSettings,
        "demo",
        tmp_cwd.path(),
        tmp_cfg.path(),
    )
    .expect("clean target");
    assert_eq!(
        target,
        tmp_cwd.path().join(".coco").join("agents").join("demo.md")
    );
}

#[test]
fn resolve_target_detects_existing_file() {
    let tmp_cwd = tempfile::tempdir().unwrap();
    let tmp_cfg = tempfile::tempdir().unwrap();
    let dir = tmp_cfg.path().join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("demo.md"), "stub").unwrap();
    let err = resolve_create_target(
        AgentSource::UserSettings,
        "demo",
        tmp_cwd.path(),
        tmp_cfg.path(),
    )
    .expect_err("must detect existing file");
    match err {
        WizardError::AlreadyExists { path } => {
            assert_eq!(path, dir.join("demo.md"));
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
}

#[test]
fn resolve_target_rejects_non_writable_source() {
    let tmp_cwd = tempfile::tempdir().unwrap();
    let tmp_cfg = tempfile::tempdir().unwrap();
    for source in [
        AgentSource::BuiltIn,
        AgentSource::Plugin,
        AgentSource::FlagSettings,
        AgentSource::PolicySettings,
    ] {
        let err = resolve_create_target(source, "demo", tmp_cwd.path(), tmp_cfg.path())
            .expect_err("non-writable must error");
        assert_eq!(err, WizardError::NonWritableSource, "{source:?}");
    }
}
