//! View-string snapshot tests for the `/permissions` editor renderer.
//!
//! Behaviour lives in `update/permissions_editor.test.rs` and
//! `state/permissions_editor.test.rs`; this file pins the emitted body
//! text for each tab + form step so cosmetic regressions surface as
//! `insta` diffs.

use super::*;
use crate::state::AddForm;
use crate::state::AddStep;
use crate::state::DeleteConfirm;
use crate::state::DeleteTarget;
use crate::state::PermRuleRow;
use crate::state::PermissionsEditorState;
use crate::state::WizardTextField;
use crate::theme::Theme;
use coco_types::PermissionBehavior;
use coco_types::PermissionRuleSource;
use coco_types::PermissionsEditorDir;
use coco_types::PermissionsEditorPayload;
use coco_types::PermissionsEditorRule;

fn sample_state() -> PermissionsEditorState {
    PermissionsEditorState::from_payload(PermissionsEditorPayload {
        rules: vec![
            PermissionsEditorRule {
                behavior: PermissionBehavior::Allow,
                source: PermissionRuleSource::LocalSettings,
                tool_pattern: "Bash".into(),
                rule_content: Some("git *".into()),
            },
            PermissionsEditorRule {
                behavior: PermissionBehavior::Allow,
                source: PermissionRuleSource::PolicySettings,
                tool_pattern: "Read".into(),
                rule_content: None,
            },
            PermissionsEditorRule {
                behavior: PermissionBehavior::Deny,
                source: PermissionRuleSource::ProjectSettings,
                tool_pattern: "Bash".into(),
                rule_content: Some("rm *".into()),
            },
        ],
        directories: vec![PermissionsEditorDir {
            path: "/srv/data".into(),
            source: PermissionRuleSource::ProjectSettings,
        }],
        cwd: "/work".into(),
        managed_only: false,
    })
}

fn body_only(state: &PermissionsEditorState) -> String {
    let _locale = crate::i18n::locale_test_guard("en");
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let (_title, body, _color) = permissions_editor_content(state, styles);
    body
}

#[test]
fn snapshot_allow_tab_list() {
    let state = sample_state();
    insta::assert_snapshot!("perms_allow_list", body_only(&state));
}

#[test]
fn snapshot_workspace_tab_list() {
    let mut state = sample_state();
    state.selected_tab = PermissionsEditorTab::Workspace;
    insta::assert_snapshot!("perms_workspace_list", body_only(&state));
}

#[test]
fn snapshot_add_form_input_step() {
    let mut state = sample_state();
    let mut form = AddForm::new();
    form.input = WizardTextField::seeded("Bash(npm *)");
    state.add_form = Some(form);
    insta::assert_snapshot!("perms_add_input", body_only(&state));
}

#[test]
fn snapshot_add_form_destination_step() {
    let mut state = sample_state();
    let mut form = AddForm::new();
    form.input = WizardTextField::seeded("Bash(npm *)");
    form.step = AddStep::Destination;
    form.destination = 1; // Project
    state.add_form = Some(form);
    insta::assert_snapshot!("perms_add_destination", body_only(&state));
}

#[test]
fn snapshot_delete_confirm() {
    let mut state = sample_state();
    state.delete_confirm = Some(DeleteConfirm {
        yes: false,
        target: DeleteTarget::Rule(PermRuleRow {
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::LocalSettings,
            tool_pattern: "Bash".into(),
            rule_content: Some("git *".into()),
        }),
    });
    insta::assert_snapshot!("perms_delete_confirm", body_only(&state));
}

#[test]
fn snapshot_managed_only_banner() {
    let mut state = sample_state();
    state.managed_only = true;
    insta::assert_snapshot!("perms_managed_only", body_only(&state));
}
