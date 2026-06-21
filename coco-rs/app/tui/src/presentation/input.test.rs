use super::*;

use crate::state::ActiveSuggestions;
use crate::state::AppState;
use crate::state::SuggestionKind;
use crate::widgets::suggestion_popup::SuggestionItem;

fn item(label: &str) -> SuggestionItem {
    SuggestionItem {
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
fn inline_popup_view_returns_none_when_no_rows_match() {
    let mut state = AppState::default();
    state.ui.completion.active = Some(ActiveSuggestions {
        kind: SuggestionKind::SlashCommand,
        items: Vec::new(),
        selected: 0,
        query: "zzz".into(),
        trigger_pos: 0,
    });
    state.ui.sync_popup_from_active_suggestions();

    assert!(inline_popup_view(&state).is_none());
}
