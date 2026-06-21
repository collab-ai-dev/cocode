use super::KeybindingHandle;
use super::ResolverResult;
use super::context_stack;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use coco_keybindings::KeybindingAction;
use coco_keybindings::KeybindingContext as KbContext;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[test]
fn ctrl_c_in_chat_resolves_to_app_interrupt() {
    let handle = KeybindingHandle::from_defaults();
    let result = handle.resolve_key(
        key(KeyCode::Char('c'), KeyModifiers::CONTROL),
        TuiContext::Chat,
    );
    match result {
        ResolverResult::Action(KeybindingAction::AppInterrupt) => {}
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn enter_in_chat_resolves_to_chat_submit() {
    let handle = KeybindingHandle::from_defaults();
    let result = handle.resolve_key(key(KeyCode::Enter, KeyModifiers::NONE), TuiContext::Chat);
    match result {
        ResolverResult::Action(KeybindingAction::ChatSubmit) => {}
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn unmapped_key_returns_not_resolved() {
    let handle = KeybindingHandle::from_defaults();
    // F6 isn't in the TS default table — resolver should bow out so
    // the legacy cascade can handle it.
    let result = handle.resolve_key(key(KeyCode::F(6), KeyModifiers::NONE), TuiContext::Chat);
    assert!(matches!(result, ResolverResult::NotResolved));
}

#[test]
fn context_stack_for_chat_includes_task_and_global() {
    // Task sits between Chat and Global so ctrl+b → TaskBackground
    // resolves while typing in the composer (defaults.rs:200).
    let stack = context_stack(TuiContext::Chat);
    assert_eq!(
        stack,
        vec![KbContext::Chat, KbContext::Task, KbContext::Global]
    );
}

#[test]
fn context_stack_for_confirmation_excludes_chat() {
    let stack = context_stack(TuiContext::Confirmation);
    assert!(!stack.contains(&KbContext::Chat));
    assert!(stack.contains(&KbContext::Confirmation));
    assert!(stack.contains(&KbContext::Global));
}

#[test]
fn handle_exposes_pending_display_after_chord_prefix() {
    let handle = KeybindingHandle::from_defaults();
    // ctrl+x is the prefix of ctrl+x ctrl+k (kill agents) — feeding
    // it should put the resolver in Pending.
    let result = handle.resolve_key(
        key(KeyCode::Char('x'), KeyModifiers::CONTROL),
        TuiContext::Chat,
    );
    assert!(matches!(result, ResolverResult::Pending));
    assert!(handle.has_pending_chord());
    let hint = handle.pending_display().unwrap();
    assert!(hint.contains("ctrl+x"));
    assert!(hint.ends_with("…"));
}

#[test]
fn display_for_returns_default_chord() {
    let handle = KeybindingHandle::from_defaults();
    let display = handle
        .display_for(&KeybindingAction::ChatSubmit, TuiContext::Chat)
        .unwrap();
    assert_eq!(display, "Enter");
}

#[test]
fn display_for_returns_plan_editor_chord() {
    let handle = KeybindingHandle::from_defaults();
    let display = handle
        .display_for(&KeybindingAction::AppPlanEditor, TuiContext::Chat)
        .unwrap();
    assert_eq!(display, "ctrl+g");
}

#[test]
fn warnings_default_to_empty() {
    let handle = KeybindingHandle::from_defaults();
    assert!(handle.warnings().is_empty());
}
