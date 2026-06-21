use super::*;
use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use coco_types::PermissionsEditorDir;
use coco_types::PermissionsEditorPayload;
use coco_types::PermissionsEditorRule;
use pretty_assertions::assert_eq;

fn rule(
    behavior: PermissionBehavior,
    source: PermissionRuleSource,
    pattern: &str,
    content: Option<&str>,
) -> PermissionsEditorRule {
    PermissionsEditorRule {
        behavior,
        source,
        tool_pattern: pattern.to_string(),
        rule_content: content.map(str::to_string),
    }
}

fn sample_payload() -> PermissionsEditorPayload {
    PermissionsEditorPayload {
        rules: vec![
            rule(
                PermissionBehavior::Allow,
                PermissionRuleSource::LocalSettings,
                "Bash",
                Some("git *"),
            ),
            rule(
                PermissionBehavior::Allow,
                PermissionRuleSource::PolicySettings,
                "Read",
                None,
            ),
            rule(
                PermissionBehavior::Deny,
                PermissionRuleSource::ProjectSettings,
                "Bash",
                Some("rm *"),
            ),
        ],
        directories: vec![PermissionsEditorDir {
            path: "/tmp/extra".to_string(),
            source: PermissionRuleSource::UserSettings,
        }],
        cwd: "/work".to_string(),
        managed_only: false,
    }
}

#[test]
fn tab_cycles_wrap_both_directions() {
    assert_eq!(
        PermissionsEditorTab::Allow.cycled(1),
        PermissionsEditorTab::Ask
    );
    assert_eq!(
        PermissionsEditorTab::Workspace.cycled(1),
        PermissionsEditorTab::Allow
    );
    assert_eq!(
        PermissionsEditorTab::Allow.cycled(-1),
        PermissionsEditorTab::Workspace
    );
}

#[test]
fn behavior_maps_tabs_to_rule_behavior() {
    assert_eq!(
        PermissionsEditorTab::Allow.behavior(),
        Some(PermissionBehavior::Allow)
    );
    assert_eq!(
        PermissionsEditorTab::Deny.behavior(),
        Some(PermissionBehavior::Deny)
    );
    assert_eq!(PermissionsEditorTab::Workspace.behavior(), None);
}

#[test]
fn source_destination_only_writable_layers() {
    assert!(source_destination(PermissionRuleSource::LocalSettings).is_some());
    assert!(source_destination(PermissionRuleSource::ProjectSettings).is_some());
    assert!(source_destination(PermissionRuleSource::UserSettings).is_some());
    assert!(source_destination(PermissionRuleSource::PolicySettings).is_none());
    assert!(source_destination(PermissionRuleSource::Session).is_none());
    assert!(source_destination(PermissionRuleSource::CliArg).is_none());
}

#[test]
fn rule_row_display_renders_specifier() {
    let with = PermRuleRow {
        behavior: PermissionBehavior::Allow,
        source: PermissionRuleSource::LocalSettings,
        tool_pattern: "Bash".into(),
        rule_content: Some("git *".into()),
    };
    let without = PermRuleRow {
        behavior: PermissionBehavior::Allow,
        source: PermissionRuleSource::LocalSettings,
        tool_pattern: "Read".into(),
        rule_content: None,
    };
    assert_eq!(with.display(), "Bash(git *)");
    assert_eq!(without.display(), "Read");
}

#[test]
fn policy_rule_is_read_only() {
    let policy = PermRuleRow {
        behavior: PermissionBehavior::Allow,
        source: PermissionRuleSource::PolicySettings,
        tool_pattern: "Read".into(),
        rule_content: None,
    };
    assert!(!policy.is_editable());
}

#[test]
fn from_payload_prepends_readonly_cwd_row() {
    let state = PermissionsEditorState::from_payload(sample_payload());
    assert_eq!(state.selected_tab, PermissionsEditorTab::Allow);
    // cwd row first, then the one settings dir.
    assert_eq!(state.directories.len(), 2);
    assert!(state.directories[0].is_cwd);
    assert_eq!(state.directories[0].path, "/work");
    assert!(!state.directories[0].is_editable());
    assert!(!state.directories[1].is_cwd);
    assert!(state.directories[1].is_editable());
}

#[test]
fn rules_for_partitions_by_behavior() {
    let state = PermissionsEditorState::from_payload(sample_payload());
    assert_eq!(state.rules_for(PermissionBehavior::Allow).len(), 2);
    assert_eq!(state.rules_for(PermissionBehavior::Deny).len(), 1);
    assert_eq!(state.rules_for(PermissionBehavior::Ask).len(), 0);
}

#[test]
fn active_len_includes_add_sentinel() {
    let state = PermissionsEditorState::from_payload(sample_payload());
    // Allow tab: 1 sentinel + 2 allow rules.
    assert_eq!(state.active_len(), 3);
}

#[test]
fn nav_clamps_at_ends_no_wrap() {
    let mut state = PermissionsEditorState::from_payload(sample_payload());
    // Allow tab has 3 rows (0..2).
    assert!(!state.nav(-1)); // already at 0
    assert!(state.nav(1));
    assert_eq!(state.active_cursor(), 1);
    assert!(state.nav(1));
    assert_eq!(state.active_cursor(), 2);
    assert!(!state.nav(1)); // clamp at max
    assert_eq!(state.active_cursor(), 2);
}

#[test]
fn focused_resolves_add_then_rules() {
    let state = PermissionsEditorState::from_payload(sample_payload());
    assert!(matches!(state.focused(), Focused::Add));
    let mut state = state;
    state.nav(1);
    assert!(matches!(state.focused(), Focused::Rule(_)));
}

#[test]
fn refresh_preserves_tab_and_clamps_cursor() {
    let mut state = PermissionsEditorState::from_payload(sample_payload());
    state.selected_tab = PermissionsEditorTab::Deny;
    state.deny_cursor = 1; // on the single deny rule
    // Refresh with an empty rule set — cursor must clamp.
    state.refresh_from_payload(PermissionsEditorPayload {
        rules: vec![],
        directories: vec![],
        cwd: "/work".to_string(),
        managed_only: false,
    });
    assert_eq!(state.selected_tab, PermissionsEditorTab::Deny);
    assert_eq!(state.deny_cursor, 0);
    assert!(state.add_form.is_none());
    assert!(state.delete_confirm.is_none());
}

#[test]
fn add_form_destination_nav_wraps() {
    let mut form = AddForm::new();
    assert_eq!(form.selected_destination(), EditorDestination::Local);
    form.nav_destination(1);
    assert_eq!(form.selected_destination(), EditorDestination::Project);
    form.nav_destination(-1);
    assert_eq!(form.selected_destination(), EditorDestination::Local);
    form.nav_destination(-1);
    assert_eq!(form.selected_destination(), EditorDestination::User);
}
