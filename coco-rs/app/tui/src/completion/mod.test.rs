use pretty_assertions::assert_eq;

use super::*;
use crate::widgets::suggestion_popup::SuggestionItem;

fn item(label: &str) -> SuggestionItem {
    SuggestionItem {
        label: label.into(),
        description: None,
        metadata: None,
    }
}

fn suggestions(
    kind: SuggestionKind,
    items: Vec<SuggestionItem>,
    trigger_pos: usize,
) -> ActiveSuggestions {
    ActiveSuggestions {
        kind,
        items,
        selected: 0,
        query: String::new(),
        trigger_pos,
    }
}

#[test]
fn test_set_active_nonempty_items_sets_had_items() {
    let mut state = CompletionState::default();

    state.set_active(
        suggestions(SuggestionKind::SlashCommand, vec![item("/clear")], 0),
        0..2,
        "/c".into(),
    );

    assert_eq!(state.had_items, true);
}

#[test]
fn test_set_active_same_session_keeps_had_items_across_empty_refresh() {
    // `/cle` shows rows, `/cleax` overshoots to zero: the session (same kind
    // + trigger position) keeps had_items so the popup slot stays reserved.
    let mut state = CompletionState::default();
    state.set_active(
        suggestions(SuggestionKind::SlashCommand, vec![item("/clear")], 0),
        0..4,
        "/cle".into(),
    );

    state.set_active(
        suggestions(SuggestionKind::SlashCommand, Vec::new(), 0),
        0..6,
        "/cleax".into(),
    );

    assert_eq!(state.had_items, true);
}

#[test]
fn test_set_active_new_session_resets_had_items() {
    // A new trigger position is a new session: an empty first result must not
    // inherit the previous session's had_items.
    let mut state = CompletionState::default();
    state.set_active(
        suggestions(SuggestionKind::SlashCommand, vec![item("/clear")], 0),
        0..2,
        "/c".into(),
    );

    state.set_active(
        suggestions(SuggestionKind::At, Vec::new(), 7),
        7..9,
        "@x".into(),
    );

    assert_eq!(state.had_items, false);
}

#[test]
fn test_set_active_kind_flip_at_same_trigger_keeps_had_items() {
    // `@` (At, agent rows shown) → `@./` (query turns path-like, kind flips
    // to Path, items empty until the async search lands): same trigger
    // position = same session, so the slot must stay reserved across the gap.
    let mut state = CompletionState::default();
    state.set_active(
        suggestions(SuggestionKind::At, vec![item("Plan (agent)")], 3),
        3..4,
        "@".into(),
    );

    state.set_active(
        suggestions(SuggestionKind::Path, Vec::new(), 3),
        3..6,
        "@./".into(),
    );

    assert_eq!(state.had_items, true);
}

#[test]
fn test_accept_directory_drill_keeps_had_items() {
    // Tab on a directory row (Drill: `@sr` → `@src/`) continues the session
    // at the same trigger position; the async re-search leaves items empty
    // for a moment, and the reserved popup slot must survive that gap.
    let mut state = crate::state::AppState::default();
    state.ui.input.textarea.set_text("@sr");
    state.ui.input.textarea.set_cursor(3);
    state.ui.completion.set_active(
        ActiveSuggestions {
            kind: SuggestionKind::At,
            items: vec![SuggestionItem {
                label: "src/".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path { is_directory: true }),
            }],
            selected: 0,
            query: "sr".into(),
            trigger_pos: 0,
        },
        0..3,
        "@sr".into(),
    );
    state.ui.sync_popup_from_active_suggestions();

    let insertion =
        accept_suggestion(&mut state, AcceptMode::ExtendCommonPrefix).expect("drill accept");

    assert_eq!(insertion.keep_popup, true);
    assert_eq!(state.ui.completion.had_items, true);
    assert!(
        crate::presentation::input::inline_popup_view(&state).is_some(),
        "popup slot must stay reserved across the drill's async result gap"
    );
}

#[test]
fn test_clear_active_resets_had_items() {
    let mut state = CompletionState::default();
    state.set_active(
        suggestions(SuggestionKind::SlashCommand, vec![item("/clear")], 0),
        0..2,
        "/c".into(),
    );

    state.clear_active();

    assert_eq!(state.had_items, false);
}
