use super::*;

use crate::i18n::locale_test_guard;
use crate::state::AppState;
use crate::theme::Theme;

fn side_chat_state(busy: bool) -> AppState {
    let mut state = AppState::default();
    let parent_id = coco_types::SessionId::try_new("parent").expect("valid parent session id");
    let child_id = coco_types::SessionId::try_new("child").expect("valid child session id");
    state.session.session_id = Some(parent_id.as_str().to_string());
    assert!(state.enter_side_chat(parent_id, child_id));
    state.session.set_busy(busy);
    state
}

fn hint_text(state: &AppState, width: u16) -> String {
    let theme = Theme::default();
    side_chat_hint_line(state, UiStyles::new(&theme), width)
        .expect("side chat should render a hint")
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn side_chat_hint_is_state_aware_and_responsive() {
    let _locale = locale_test_guard("en");

    let idle = side_chat_state(false);
    assert_eq!(hint_text(&idle, 80), "Side chat · Ctrl+C to return");
    assert_eq!(hint_text(&idle, 50), "Ctrl+C to return");
    assert_eq!(hint_text(&idle, 40), "Ctrl+C ↩ main");
    assert_eq!(hint_text(&idle, 30), "Ctrl+C");

    let busy = side_chat_state(true);
    assert_eq!(hint_text(&busy, 80), "Side chat · Ctrl+C to interrupt");
}

#[test]
fn regular_chat_has_no_side_chat_hint() {
    let _locale = locale_test_guard("en");
    let state = AppState::default();
    let theme = Theme::default();

    assert!(side_chat_hint_line(&state, UiStyles::new(&theme), 80).is_none());
}
