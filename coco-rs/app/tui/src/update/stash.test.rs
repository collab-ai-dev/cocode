use super::swap_input_draft;
use crate::state::AppState;
use crate::state::ui::StashedInput;

#[test]
fn empty_input_with_no_stash_is_silent_noop() {
    let mut state = AppState::new();
    swap_input_draft(&mut state);
    assert!(state.ui.input.text().is_empty());
    assert!(state.ui.stashed_input.is_none());
    // No toast on the silent no-op.
    assert!(state.ui.toasts.is_empty());
}

#[test]
fn non_empty_input_pushes_to_stash_and_clears_input() {
    let mut state = AppState::new();
    state.ui.input.textarea_mut().set_text("hello world");
    state.ui.input.textarea_mut().set_cursor(5);

    swap_input_draft(&mut state);

    assert_eq!(state.ui.input.text(), "");
    assert_eq!(state.ui.input.textarea().cursor(), 0);
    let stash = state.ui.stashed_input.as_ref().expect("stash present");
    assert_eq!(stash.composer.text(), "hello world");
    assert_eq!(stash.composer.cursor(), 5);
}

#[test]
fn empty_input_with_stash_pops_stash_into_input() {
    let mut state = AppState::new();
    state.ui.stashed_input = Some(StashedInput::plain("stashed", 7));

    swap_input_draft(&mut state);

    assert_eq!(state.ui.input.text(), "stashed");
    assert_eq!(state.ui.input.textarea().cursor(), 7);
    assert!(state.ui.stashed_input.is_none());
}

#[test]
fn stash_round_trips_paste_entries() {
    let mut state = AppState::new();
    state.ui.input.textarea_mut().insert_str("hello ");
    state
        .ui
        .input
        .insert_text_attachment("first paste".into())
        .unwrap();
    state.ui.input.textarea_mut().insert_str(" world");
    let eol = state.ui.input.textarea().end_of_current_line();
    state.ui.input.textarea_mut().set_cursor(eol);

    // Push: paste entries move into the stash slot.
    swap_input_draft(&mut state);
    assert!(state.ui.input.text().is_empty());
    assert!(state.ui.input.attachments_empty());
    let stash = state.ui.stashed_input.as_ref().expect("pushed");
    assert_eq!(stash.composer.attachments().len(), 1);
    assert!(stash.composer.text().contains("[Pasted text #1]"));

    // Pop: paste entries restored alongside text + cursor so pills
    // still resolve.
    swap_input_draft(&mut state);
    assert!(state.ui.input.text().contains("[Pasted text #1]"));
    assert_eq!(state.ui.input.attachment_count(), 1);
    let resolved = state.ui.input.resolve().unwrap();
    assert!(resolved.text.contains("first paste"));
}

#[test]
fn stash_round_trips_atomic_file_refs() {
    use coco_tui_ui::widgets::ElementDisplay;
    use coco_tui_ui::widgets::ElementKind;
    use ratatui::style::Style;

    let mut state = AppState::new();
    assert!(
        state
            .ui
            .input
            .textarea_mut()
            .insert_element(
                "@src/main.rs",
                ElementKind::FileRef,
                ElementDisplay::new("@src/main.rs", Style::new().underlined()),
            )
            .is_ok()
    );

    swap_input_draft(&mut state);
    assert!(state.ui.input.text().is_empty());
    swap_input_draft(&mut state);

    assert_eq!(state.ui.input.text(), "@src/main.rs");
    assert_eq!(state.ui.input.textarea().elements().len(), 1);
    assert_eq!(
        state.ui.input.textarea().elements()[0].kind(),
        ElementKind::FileRef
    );
}

#[test]
fn non_empty_input_overwrites_existing_stash() {
    // Pushing with a prior stash does NOT swap —
    // the prior stash is overwritten. There is no stash list.
    let mut state = AppState::new();
    state.ui.stashed_input = Some(StashedInput::plain("old", 3));
    state.ui.input.textarea_mut().set_text("new");
    state.ui.input.textarea_mut().set_cursor(3);

    swap_input_draft(&mut state);

    let stash = state.ui.stashed_input.as_ref().expect("stash present");
    assert_eq!(
        stash.composer.text(),
        "new",
        "push overwrites the prior stash",
    );
    assert!(state.ui.input.text().is_empty());
}

#[test]
fn whitespace_only_input_is_treated_as_empty() {
    // TS uses `input.trim() === ''`, so all-whitespace input pops
    // the stash (or no-ops) rather than pushing.
    let mut state = AppState::new();
    state.ui.input.textarea_mut().set_text("   \n  ");
    state.ui.stashed_input = Some(StashedInput::plain("real draft", 10));

    swap_input_draft(&mut state);

    assert_eq!(state.ui.input.text(), "real draft");
    assert!(state.ui.stashed_input.is_none());
}
