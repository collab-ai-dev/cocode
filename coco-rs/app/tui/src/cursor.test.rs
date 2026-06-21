use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::*;
use crate::state::AppState;

/// Helper: build a minimal AppState with input focused and given text.
fn make_state(text: &str) -> AppState {
    let mut state = AppState::default();
    state.ui.focus = FocusTarget::Input;
    if !text.is_empty() {
        state.ui.input.textarea.insert_str(text);
    }
    state
}

const INPUT_AREA: Rect = Rect {
    x: 0,
    y: 10,
    width: 80,
    height: 3,
};

fn layout(input: Rect) -> FrameLayout {
    FrameLayout {
        input,
        question_prompt: Rect::default(),
    }
}

#[test]
fn compute_cursor_returns_none_when_input_not_focused() {
    let mut state = AppState::default();
    state.ui.focus = FocusTarget::Chat;
    assert!(compute_cursor(&state, layout(INPUT_AREA)).is_none());
}

#[test]
fn compute_cursor_returns_some_for_empty_input_when_focused() {
    // This is the regression: empty input must still return a claim so
    // the post-draw pin has somewhere to put the cursor; otherwise
    // focus-gained re-shows it in the status bar.
    let state = make_state("");
    let claim = compute_cursor(&state, layout(INPUT_AREA)).expect("empty input must claim cursor");
    // indicator_width=2 → cursor sits in column 2, second row of input area.
    assert_eq!(claim.position.x, INPUT_AREA.x + 2);
    assert_eq!(claim.position.y, INPUT_AREA.y + 1);
}

#[test]
fn compute_cursor_returns_none_when_modal_owns_focus() {
    let mut state = make_state("hello");
    state.ui.show_modal(crate::state::ModalState::Help);

    assert!(compute_cursor(&state, layout(INPUT_AREA)).is_none());
}

#[test]
fn compute_cursor_advances_past_ascii_text() {
    let state = make_state("hello");
    let claim = compute_cursor(&state, layout(INPUT_AREA)).unwrap();
    assert_eq!(claim.position.x, INPUT_AREA.x + 2 + 5);
    assert_eq!(claim.position.y, INPUT_AREA.y + 1);
}

#[test]
fn compute_cursor_handles_cjk_width() {
    // "你好" is 4 display columns, not 2.
    let state = make_state("你好");
    let claim = compute_cursor(&state, layout(INPUT_AREA)).unwrap();
    assert_eq!(claim.position.x, INPUT_AREA.x + 2 + 4);
}

#[test]
fn compute_cursor_returns_none_for_zero_sized_area() {
    let state = make_state("hi");
    let zero = Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
    assert!(compute_cursor(&state, layout(zero)).is_none());
}

#[test]
fn compute_cursor_clamps_to_area_width() {
    let state = make_state(&"x".repeat(200));
    let claim = compute_cursor(&state, layout(INPUT_AREA)).unwrap();
    // max_cursor = width - (indicator_width + 1) = 80 - 3 = 77
    // cursor_x = area.x + indicator_width + min(raw, 77) = 0 + 2 + 77 = 79
    assert_eq!(claim.position.x, INPUT_AREA.x + 2 + 77);
}

#[test]
fn compute_cursor_uses_focused_question_input() {
    let mut state = AppState::default();
    state.ui.focus = FocusTarget::Input;
    state
        .ui
        .push_prompt(crate::state::PanePromptState::Question(
            crate::state::QuestionPromptState {
                request_id: "req".into(),
                original_input: serde_json::json!({}),
                questions: vec![crate::state::QuestionItem {
                    header: "H".into(),
                    question: "Question?".into(),
                    options: vec![crate::state::QuestionOption {
                        label: "A".into(),
                        description: String::new(),
                        preview: None,
                    }],
                    multi_select: false,
                    selected: None,
                    checked: Vec::new(),
                    other_input: crate::state::OtherInputState {
                        focused: true,
                        value: "hello".into(),
                        committed: false,
                    },
                }],
                current_question: crate::state::QuestionPage::Question(0),
                focus_target: crate::state::QuestionFocusTarget::OtherInput,
                is_in_plan_mode: false,
            },
        ));
    let question_prompt = Rect::new(5, 3, 40, 12);
    let claim = compute_cursor(
        &state,
        FrameLayout {
            input: INPUT_AREA,
            question_prompt,
        },
    )
    .expect("focused question input claims cursor");

    assert!(claim.position.x > question_prompt.x);
    assert!(claim.position.y > question_prompt.y);
    assert!(claim.position.y < question_prompt.bottom());
}
