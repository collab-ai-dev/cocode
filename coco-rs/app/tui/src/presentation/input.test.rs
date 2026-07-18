use super::*;

use crate::state::ActiveSuggestions;
use crate::state::AppState;
use crate::state::SuggestionKind;
use crate::widgets::suggestion_popup::SuggestionItem;

fn item(label: &str) -> SuggestionItem {
    SuggestionItem {
        highlight_indices: Vec::new(),
        label: label.into(),
        description: None,
        metadata: None,
    }
}

#[test]
fn inline_popup_view_reads_interaction_popup() {
    let mut state = AppState::default();
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![item("/help")],
        selected: 2,
        query: String::new(),
        trigger_pos: 0,
    });
    state.ui.sync_popup_from_active_suggestions();

    let view = inline_popup_view(&state).expect("interaction popup should render");

    assert_eq!(view.selected, 2);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].label, "/help");
}

#[test]
fn inline_popup_view_filters_command_palette_items() {
    let mut state = AppState::default();
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: vec![SuggestionItem {
            highlight_indices: Vec::new(),
            label: "/clear".into(),
            description: Some("Clear chat".into()),
            metadata: None,
        }],
        selected: 0,
        query: "cle".into(),
        trigger_pos: 0,
    });
    state.ui.sync_popup_from_active_suggestions();

    let view = inline_popup_view(&state).expect("matching command should render");

    assert_eq!(view.selected, 0);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].label, "/clear");
    assert_eq!(view.items[0].description.as_deref(), Some("Clear chat"));
}

#[test]
fn inline_popup_view_stays_visible_when_filter_overshoots() {
    // An overshot filter (e.g. `/clea` → `/cleax`) keeps the session — and
    // the reserved popup slot — alive so the composer doesn't
    // collapse-and-reopen while the user corrects the query. The session
    // qualifies because it has already shown rows (`had_items`).
    let mut state = AppState::default();
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: Vec::new(),
        selected: 0,
        query: "cleax".into(),
        trigger_pos: 0,
    });
    state.ui.completion.had_items = true;
    state.ui.sync_popup_from_active_suggestions();

    let view = inline_popup_view(&state).expect("overshot session should stay visible");
    assert!(view.items.is_empty());
}

#[test]
fn inline_popup_view_returns_none_when_session_never_matched() {
    // A trigger false-positive that never produced a row (bash token with a
    // `/`, a `/usr/...` path typed as a message) must not materialize a
    // "no matches" panel.
    let mut state = AppState::default();
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: Vec::new(),
        selected: 0,
        query: "usr/bin/env".into(),
        trigger_pos: 0,
    });
    state.ui.sync_popup_from_active_suggestions();

    assert!(inline_popup_view(&state).is_none());
}
