use super::OtherInputState;
use super::QuestionFocusTarget;
use super::QuestionItem;
use super::QuestionOption;
use super::QuestionPage;
use super::QuestionPromptState;

fn opt(label: &str) -> QuestionOption {
    QuestionOption {
        label: label.into(),
        description: String::new(),
        preview: None,
    }
}

fn q(text: &str, selected: usize, options: Vec<QuestionOption>) -> QuestionItem {
    QuestionItem {
        header: "h".into(),
        question: text.into(),
        options,
        multi_select: false,
        selected: Some(selected),
        checked: Vec::new(),
        other_input: OtherInputState::default(),
    }
}

fn state(questions: Vec<QuestionItem>, plan_mode: bool) -> QuestionPromptState {
    QuestionPromptState {
        request_id: "rid".into(),
        original_input: serde_json::json!({}),
        questions,
        current_question: QuestionPage::Question(0),
        focus_target: QuestionFocusTarget::QuestionOption(0),
        is_in_plan_mode: plan_mode,
    }
}

#[test]
fn chat_about_this_matches_ts_with_partial_answers() {
    let mut o = state(
        vec![
            q("Which library?", 0, vec![opt("Tokio"), opt("Async-std")]),
            q("Custom name?", 0, vec![opt("Default")]),
        ],
        false,
    );
    o.questions[1].other_input.focused = true;

    let actual = o.chat_about_this_feedback();
    let expected = "\
The user wants to clarify these questions.\n    \
This means they may have additional information, context or questions for you.\n    \
Take their response into account and then reformulate the questions if appropriate.\n    \
Start by asking them what they would like to clarify.\n\n    \
Questions asked:\n\
- \"Which library?\"\n  Answer: Tokio\n\
- \"Custom name?\"\n  (No answer provided)";

    pretty_assertions::assert_eq!(actual, expected);
}

#[test]
fn skip_interview_matches_ts() {
    let o = state(
        vec![q("Approach?", 0, vec![opt("Refactor"), opt("Rewrite")])],
        true,
    );

    let actual = o.skip_interview_feedback();
    let expected = "\
The user has indicated they have provided enough answers for the plan interview.\n\
Stop asking clarifying questions and proceed to finish the plan with the information you have.\n\n\
Questions asked and answers provided:\n\
- \"Approach?\"\n  Answer: Refactor";

    pretty_assertions::assert_eq!(actual, expected);
}

#[test]
fn other_input_with_text_uses_typed_text_as_answer() {
    let mut o = state(vec![q("Pick:", 0, vec![opt("Tokio")])], false);
    o.questions[0].other_input.value = "  rayon  ".into();

    let actual = o.chat_about_this_feedback();
    assert!(
        actual.contains("Answer: rayon"),
        "free-text input must trim and use typed text; got: {actual}"
    );
    assert!(
        !actual.contains("Answer: Tokio"),
        "typed text should override the selected pick; got: {actual}"
    );
}

#[test]
fn multi_select_joins_checked_labels_with_comma_space() {
    let mut item = q("Pick many:", 0, vec![opt("A"), opt("B"), opt("C")]);
    item.multi_select = true;
    item.checked = vec![0, 2];
    let o = state(vec![item], false);

    let actual = o.chat_about_this_feedback();
    assert!(actual.contains("Answer: A, C"), "got: {actual}");
}

#[test]
fn no_answer_when_free_text_focused_with_no_value() {
    let mut o = state(vec![q("Q?", 0, vec![opt("Skip")])], false);
    o.questions[0].other_input.focused = true;
    o.focus_target = QuestionFocusTarget::OtherInput;
    let actual = o.chat_about_this_feedback();
    assert!(actual.contains("(No answer provided)"), "got: {actual}");
}

#[test]
fn is_editing_tracks_focused_free_text_input() {
    let mut item = q("Q?", 0, vec![opt("Pick")]);
    item.other_input.focused = true;
    assert!(item.is_editing(), "Other focused must report editing");
    item.other_input.focused = false;
    assert!(!item.is_editing(), "normal pick focused must not edit");
}
