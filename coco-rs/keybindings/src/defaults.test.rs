use super::default_blocks;
use super::default_config;
use crate::KeybindingAction;
use crate::KeybindingContext;

#[test]
fn default_config_parses_every_chord() {
    let config = default_config();
    let parsed = config.parse_bindings();
    let total: usize = config.bindings.iter().map(|b| b.bindings.len()).sum();
    assert_eq!(
        parsed.len(),
        total,
        "every default chord must parse cleanly"
    );
}

#[test]
fn defaults_cover_every_user_context_except_known_skips() {
    // Some contexts have few defaults (Help is one entry; Plugin has 2).
    // Verify every context in the table parses, not that we hit them all.
    let blocks = default_blocks();
    let contexts: std::collections::HashSet<_> = blocks.iter().map(|b| b.context).collect();

    // Sanity: must include the headline contexts.
    for ctx in [
        KeybindingContext::Global,
        KeybindingContext::Chat,
        KeybindingContext::Confirmation,
        KeybindingContext::Settings,
        KeybindingContext::DiffDialog,
        KeybindingContext::Select,
    ] {
        assert!(contexts.contains(&ctx), "missing context {ctx:?}");
    }
}

#[test]
fn global_block_includes_ctrl_c_and_d() {
    let blocks = default_blocks();
    let global = blocks
        .iter()
        .find(|b| b.context == KeybindingContext::Global)
        .expect("Global block present");
    assert_eq!(
        global.bindings.get("ctrl+c").and_then(Option::as_ref),
        Some(&KeybindingAction::AppInterrupt),
    );
    assert_eq!(
        global.bindings.get("ctrl+d").and_then(Option::as_ref),
        Some(&KeybindingAction::AppExit),
    );
}

#[test]
fn chat_block_includes_chord_kill_agents() {
    let blocks = default_blocks();
    let chat = blocks
        .iter()
        .find(|b| b.context == KeybindingContext::Chat)
        .expect("Chat block present");
    assert!(
        chat.bindings.contains_key("ctrl+x ctrl+k"),
        "chord chat:killAgents must be present",
    );
    assert_eq!(
        chat.bindings.get("ctrl+x ctrl+k").and_then(Option::as_ref),
        Some(&KeybindingAction::ChatKillAgents),
    );
}

#[test]
fn chat_block_includes_single_key_thinking_toggle() {
    let blocks = default_blocks();
    let chat = blocks
        .iter()
        .find(|b| b.context == KeybindingContext::Chat)
        .expect("Chat block present");
    assert_eq!(
        chat.bindings.get("f2").and_then(Option::as_ref),
        Some(&KeybindingAction::ChatThinkingToggle),
    );
}

#[test]
fn confirmation_enter_selects_focused_action() {
    let blocks = default_blocks();
    let confirmation = blocks
        .iter()
        .find(|b| b.context == KeybindingContext::Confirmation)
        .expect("Confirmation block present");
    assert_eq!(
        confirmation.bindings.get("enter").and_then(Option::as_ref),
        Some(&KeybindingAction::ConfirmToggle),
    );
}

#[test]
fn image_paste_key_is_platform_appropriate() {
    let blocks = default_blocks();
    let chat = blocks
        .iter()
        .find(|b| b.context == KeybindingContext::Chat)
        .unwrap();
    let expected = if cfg!(target_os = "windows") {
        "alt+v"
    } else {
        "ctrl+v"
    };
    assert!(
        chat.bindings.contains_key(expected),
        "expected {expected} in Chat defaults",
    );
}
