use super::Severity;
use super::ValidationKind;
use super::format_issue;
use super::validate;
use crate::KeybindingAction;
use crate::KeybindingBlock;
use crate::KeybindingContext;
use crate::KeybindingsConfig;
use std::collections::BTreeMap;

fn config_with(blocks: Vec<KeybindingBlock>) -> KeybindingsConfig {
    KeybindingsConfig {
        schema: None,
        docs: None,
        bindings: blocks,
    }
}

fn block(
    context: KeybindingContext,
    entries: &[(&str, Option<KeybindingAction>)],
) -> KeybindingBlock {
    let mut bindings = BTreeMap::new();
    for (chord, action) in entries {
        bindings.insert((*chord).to_string(), action.clone());
    }
    KeybindingBlock { context, bindings }
}

#[test]
fn valid_config_produces_no_issues() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[
            ("ctrl+y", Some(KeybindingAction::ChatCancel)),
            ("enter", Some(KeybindingAction::ChatSubmit)),
        ],
    )]);
    assert!(validate(&config).is_empty());
}

#[test]
fn ctrl_c_in_user_config_is_reported_as_reserved() {
    // `ctrl+c` is in NON_REBINDABLE — even though the chord parses
    // and the action is valid, the validator must surface it as an
    // error.
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[("ctrl+c", Some(KeybindingAction::ChatCancel))],
    )]);
    let issues = validate(&config);
    assert!(
        issues
            .iter()
            .any(|i| i.kind == ValidationKind::Reserved && i.severity == Severity::Error),
    );
}

#[test]
fn parse_error_is_reported() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[("", Some(KeybindingAction::ChatSubmit))],
    )]);
    let issues = validate(&config);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, ValidationKind::ParseError);
    assert_eq!(issues[0].severity, Severity::Error);
}

#[test]
fn duplicate_with_conflicting_action_is_warning() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[
            ("ctrl+a", Some(KeybindingAction::ChatSubmit)),
            // Different chord-string spelling but same canonical chord.
            ("Ctrl+A", Some(KeybindingAction::ChatCancel)),
        ],
    )]);
    let issues = validate(&config);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, ValidationKind::Duplicate);
    assert_eq!(issues[0].severity, Severity::Warning);
}

#[test]
fn same_chord_same_action_is_not_a_duplicate() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[
            ("ctrl+a", Some(KeybindingAction::ChatSubmit)),
            ("Ctrl+A", Some(KeybindingAction::ChatSubmit)),
        ],
    )]);
    assert!(validate(&config).is_empty());
}

#[test]
fn different_contexts_do_not_duplicate() {
    let config = config_with(vec![
        block(
            KeybindingContext::Chat,
            &[("enter", Some(KeybindingAction::ChatSubmit))],
        ),
        block(
            KeybindingContext::Confirmation,
            &[("enter", Some(KeybindingAction::ConfirmYes))],
        ),
    ]);
    assert!(validate(&config).is_empty());
}

#[test]
fn internal_context_in_user_config_is_error() {
    let config = config_with(vec![block(
        KeybindingContext::Scroll,
        &[("up", Some(KeybindingAction::ScrollLineUp))],
    )]);
    let issues = validate(&config);
    assert!(
        issues
            .iter()
            .any(|i| { i.kind == ValidationKind::InvalidContext && i.severity == Severity::Error })
    );
}

#[test]
fn command_binding_outside_chat_warns() {
    let config = config_with(vec![block(
        KeybindingContext::Global,
        &[("ctrl+h", Some(KeybindingAction::Command("help".into())))],
    )]);
    let issues = validate(&config);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, ValidationKind::InvalidAction);
    assert_eq!(issues[0].severity, Severity::Warning);
}

#[test]
fn voice_push_to_talk_on_bare_letter_warns() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[("k", Some(KeybindingAction::VoicePushToTalk))],
    )]);
    let issues = validate(&config);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, ValidationKind::InvalidAction);
    assert_eq!(issues[0].severity, Severity::Warning);
}

#[test]
fn voice_push_to_talk_on_modifier_combo_is_ok() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[("meta+k", Some(KeybindingAction::VoicePushToTalk))],
    )]);
    assert!(validate(&config).is_empty());
}

#[test]
fn null_unbind_is_valid() {
    let config = config_with(vec![block(KeybindingContext::Chat, &[("ctrl+t", None)])]);
    assert!(validate(&config).is_empty());
}

#[test]
fn format_issue_renders_with_severity_icon_and_suggestion() {
    let config = config_with(vec![block(
        KeybindingContext::Chat,
        &[("", Some(KeybindingAction::ChatSubmit))],
    )]);
    let issues = validate(&config);
    let formatted = format_issue(&issues[0]);
    assert!(formatted.starts_with("✗ Keybinding error:"));
}
