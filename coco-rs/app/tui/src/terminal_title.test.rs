use pretty_assertions::assert_eq;

use super::title_text;
use crate::i18n::locale_test_guard;
use crate::state::AppState;
use crate::state::PermissionPromptState;
use crate::state::interaction::PanePromptState;
use coco_config::settings::TerminalTitleItem;

fn permission_prompt() -> PanePromptState {
    PanePromptState::Permission(PermissionPromptState {
        request_id: "permission-1".to_string(),
        tool_name: "Bash".to_string(),
        description: "Run command".to_string(),
        detail: crate::state::PermissionDetail::Generic {
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
        mcp_allow_scope: Default::default(),
        deny_reason_input: None,
    })
}

#[test]
fn default_items_lead_with_run_state_then_project() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.working_dir = Some("/home/dev/my-project".into());
    assert_eq!(
        title_text(&state, TerminalTitleItem::default_items()).as_deref(),
        Some("idle · my-project · cocode"),
    );
}

#[test]
fn pending_prompt_marks_the_title_as_needing_attention() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.working_dir = Some("/home/dev/my-project".into());
    state.ui.interaction.active_prompt = Some(permission_prompt());
    let title = title_text(&state, TerminalTitleItem::default_items()).expect("title");
    // The marker leads so it survives a tab bar that truncates from the right.
    assert!(title.starts_with("\u{25cf} needs input"), "{title}");
}

#[test]
fn unavailable_items_are_skipped_rather_than_rendered_empty() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();
    // No working dir and no branch: those items contribute nothing, and the
    // separators for them must not survive either.
    assert_eq!(
        title_text(
            &state,
            &[
                TerminalTitleItem::Project,
                TerminalTitleItem::GitBranch,
                TerminalTitleItem::AppName,
            ],
        )
        .as_deref(),
        Some("cocode"),
    );
}

#[test]
fn every_item_unavailable_yields_no_title() {
    let state = AppState::default();
    assert_eq!(
        title_text(
            &state,
            &[TerminalTitleItem::Project, TerminalTitleItem::GitBranch],
        ),
        None,
    );
}

#[test]
fn item_order_follows_the_configuration() {
    let _locale = locale_test_guard("en");
    let mut state = AppState::default();
    state.session.working_dir = Some("/home/dev/my-project".into());
    state.session.git_branch = Some("feat/gpt56".into());
    assert_eq!(
        title_text(
            &state,
            &[
                TerminalTitleItem::AppName,
                TerminalTitleItem::GitBranch,
                TerminalTitleItem::Project,
            ],
        )
        .as_deref(),
        Some("cocode · feat/gpt56 · my-project"),
    );
}

#[test]
fn cwd_item_renders_the_full_path() {
    let mut state = AppState::default();
    state.session.working_dir = Some("/home/dev/my-project".into());
    assert_eq!(
        title_text(&state, &[TerminalTitleItem::Cwd]).as_deref(),
        Some("/home/dev/my-project"),
    );
}
