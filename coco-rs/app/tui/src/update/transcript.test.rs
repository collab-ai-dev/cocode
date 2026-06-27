use super::toggle;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::transcript::TranscriptCellId;
use crate::state::transcript::TranscriptScrollPosition;
use crate::transcript::derive::test_helpers;

#[test]
fn toggle_opens_transcript_when_no_surface_active() {
    let mut state = AppState::new();
    toggle(&mut state);
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::Transcript(_))
    ));
}

#[test]
fn toggle_closes_transcript_when_already_open() {
    let mut state = AppState::new();
    toggle(&mut state);
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::Transcript(_))
    ));
    toggle(&mut state);
    assert!(!state.ui.has_active_surface());
}

#[test]
fn transcript_modal_defaults_to_cell_pager_state() {
    let mut state = AppState::new();
    toggle(&mut state);
    let state = state.ui.modal.as_ref().expect("transcript opened");
    let ModalState::Transcript(t) = state else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Top);
    assert_eq!(t.selected_cell_id, None);
    assert!(t.collapsed_cell_ids.is_empty());
}

#[test]
fn toggle_opens_transcript_on_latest_expandable_cell() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "old", "Read", "old\nlines", false);
    test_helpers::push_tool_result(&mut state.session, "new", "Read", "new\nlines", false);

    toggle(&mut state);

    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("new"))
    );
    assert_eq!(
        t.scroll,
        TranscriptScrollPosition::anchor(TranscriptCellId::tool("new"))
    );
}

#[test]
fn select_and_enter_toggle_collapsed_cell() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "call-1", "Read", "alpha\nbeta", false);
    toggle(&mut state);

    assert!(super::select_expandable(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("call-1"))
    );

    assert!(super::toggle_selected_cell(&mut state));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert!(
        t.collapsed_cell_ids
            .contains(&TranscriptCellId::tool("call-1"))
    );

    assert!(super::toggle_selected_cell(&mut state));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert!(
        !t.collapsed_cell_ids
            .contains(&TranscriptCellId::tool("call-1"))
    );
}

#[test]
fn select_expandable_wraps_at_edges() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "first", "Read", "one\ntwo", false);
    test_helpers::push_tool_result(&mut state.session, "last", "Read", "three\nfour", false);
    toggle(&mut state);

    assert!(super::select_expandable(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("first"))
    );

    assert!(super::select_expandable(&mut state, -1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.selected_cell_id.as_ref(),
        Some(&TranscriptCellId::tool("last"))
    );
}

#[test]
fn select_expandable_anchors_selected_cell_from_current_scroll() {
    let mut state = AppState::new();
    test_helpers::push_tool_result(&mut state.session, "call-1", "Read", "alpha\nbeta", false);
    toggle(&mut state);
    assert!(super::scroll_lines(&mut state, 40));

    assert!(super::select_expandable(&mut state, 1));

    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(
        t.scroll,
        TranscriptScrollPosition::anchor(TranscriptCellId::tool("call-1"))
    );
}

#[test]
fn transcript_scroll_uses_modal_state() {
    let mut state = AppState::new();
    toggle(&mut state);

    assert!(super::scroll_lines(&mut state, 5));
    assert_eq!(state.ui.scroll_offset, 0);
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Absolute(5));
}

#[test]
fn transcript_page_uses_terminal_size() {
    let mut state = AppState::new();
    state.ui.terminal_size = ratatui::layout::Size::new(100, 21);
    toggle(&mut state);
    let Some(ModalState::Transcript(_)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };

    assert!(super::page(&mut state, 1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Absolute(17));

    assert!(super::page(&mut state, -1));
    let Some(ModalState::Transcript(t)) = state.ui.modal.as_ref() else {
        panic!("expected Transcript state");
    };
    assert_eq!(t.scroll, TranscriptScrollPosition::Top);
}
