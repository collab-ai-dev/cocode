use pretty_assertions::assert_eq;

use super::*;
use crate::widgets::suggestion_popup::SuggestionItem;

fn item(label: &str) -> SuggestionItem {
    SuggestionItem {
        highlight_indices: Vec::new(),
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
    state.ui.input.textarea_mut().set_text("@sr");
    state.ui.input.textarea_mut().set_cursor(3);
    state.ui.completion.set_active(
        ActiveSuggestions {
            kind: SuggestionKind::At,
            items: vec![SuggestionItem {
                highlight_indices: Vec::new(),
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
fn accepting_an_at_path_creates_an_atomic_file_ref() {
    let mut state = crate::state::AppState::default();
    state.ui.input.textarea_mut().set_text("@ma");
    state.ui.input.textarea_mut().set_cursor(3);
    state.ui.completion.set_active(
        ActiveSuggestions {
            kind: SuggestionKind::At,
            items: vec![SuggestionItem {
                highlight_indices: Vec::new(),
                label: "main.rs".into(),
                description: None,
                metadata: Some(SuggestionMeta::Path {
                    is_directory: false,
                }),
            }],
            selected: 0,
            query: "ma".into(),
            trigger_pos: 0,
        },
        0..3,
        "@ma".into(),
    );

    accept_suggestion(&mut state, AcceptMode::AcceptSelected).expect("accept file");

    assert_eq!(state.ui.input.text(), "@main.rs ");
    let element = &state.ui.input.textarea().elements()[0];
    assert_eq!(element.kind(), coco_tui_ui::widgets::ElementKind::FileRef);
    let range = element.range().clone();
    assert_eq!(state.ui.input.text().get(range.clone()), Some("@main.rs"));
    state.ui.input.textarea_mut().set_cursor(range.end);
    state.ui.input.textarea_mut().move_cursor_left();
    assert_eq!(state.ui.input.textarea().cursor(), range.start);

    assert!(state.ui.input.textarea_mut().undo());
    assert_eq!(state.ui.input.text(), "@ma");
    assert!(state.ui.input.textarea().elements().is_empty());
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
