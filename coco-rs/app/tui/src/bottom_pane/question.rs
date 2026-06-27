//! AskUserQuestion prompt behavior: the key map, focus cycling, page
//! switching, digit shortcuts, multi-select toggles, the free-text "Other"
//! input, and the answer-payload submit/reject paths.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::PanePromptState;

/// Keys for the AskUserQuestion multi-choice prompt.
///
/// Unlike [`crate::bottom_pane::confirmation_map_key`] it never emits
/// `Approve`/`Deny`/`ApproveAll`: a question commits only via Enter.
/// Printable characters (and Space/Backspace) flow to `SurfaceFilter*`,
/// which [`filter_question`] routes to the multi-select toggle or the
/// question free-text input.
pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if ctrl => Some(TuiCommand::Cancel),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        // Left/Right switch between questions (wrapping). Footer actions stay on Tab.
        KeyCode::Left => Some(TuiCommand::QuestionSwitchQuestion(-1)),
        KeyCode::Right => Some(TuiCommand::QuestionSwitchQuestion(1)),
        // Tab/Shift+Tab cycle between questions and the footer actions
        // ("Chat about this" / "Skip interview"), via question_cycle_focus.
        KeyCode::Tab => Some(TuiCommand::SettingsNextTab),
        KeyCode::BackTab => Some(TuiCommand::SettingsPrevTab),
        KeyCode::Backspace => Some(TuiCommand::SurfaceFilterBackspace),
        // Every printable char (Space included) routes through the filter
        // path: Space toggles a multi-select option, other chars type into
        // the focused free-text input, and are silently swallowed otherwise.
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
            Some(TuiCommand::SurfaceFilter(c))
        }
        _ => None,
    }
}

/// Filter-keystroke routing for a Question prompt. Space toggles
/// multi-select; printable chars edit the free-text input when it is
/// focused; otherwise digits 1-9 jump to an option via number shortcuts.
/// Question prompts have no filter — everything else is silently swallowed.
pub(crate) fn filter_question(state: &mut AppState, c: char) {
    // The focused "Other" free-text input owns every printable char INCLUDING
    // space (multi-word answers) and digits; `question_free_text_input` returns
    // false unless that input is focused, so only when it is not do Space toggle
    // a multi-select option and a digit jump to an option.
    if question_free_text_input(state, c) {
        return;
    }
    if c == ' ' {
        question_toggle_checked(state);
        return;
    }
    if let Some(d) = c.to_digit(10) {
        question_select_digit(state, d as i32);
    }
}

pub(crate) async fn confirm_question_prompt(
    state: &mut AppState,
    q: crate::state::QuestionPromptState,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionFooterAction;
    use crate::state::QuestionPage;
    use crate::state::SubmitAction;

    match (q.current_question, q.focus_target) {
        (QuestionPage::Question(_), QuestionFocusTarget::OtherInput)
            if q.current_question_item()
                .is_some_and(|item| item.other_input.value.trim().is_empty()) =>
        {
            state.ui.restore_prompt(PanePromptState::Question(q));
            true
        }
        (QuestionPage::Question(_), QuestionFocusTarget::QuestionOption(_))
        | (QuestionPage::Question(_), QuestionFocusTarget::OtherInput) => {
            let mut q = q;
            q.commit_focused_answer();
            if q.advance_question_or_submit() {
                state.ui.restore_prompt(PanePromptState::Question(q));
                true
            } else {
                submit_question_answers(&q, command_tx).await;
                false
            }
        }
        (
            QuestionPage::Question(_),
            QuestionFocusTarget::QuestionFooter(QuestionFooterAction::ChatAboutThis),
        ) => {
            let feedback = q.chat_about_this_feedback();
            send_question_rejection(&q, feedback, command_tx).await;
            false
        }
        (
            QuestionPage::Question(_),
            QuestionFocusTarget::QuestionFooter(QuestionFooterAction::SkipInterview),
        ) => {
            if !q.is_in_plan_mode {
                state.ui.restore_prompt(PanePromptState::Question(q));
                return true;
            }
            let feedback = q.skip_interview_feedback();
            send_question_rejection(&q, feedback, command_tx).await;
            false
        }
        (QuestionPage::Submit, QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers)) => {
            submit_question_answers(&q, command_tx).await;
            false
        }
        (QuestionPage::Submit, QuestionFocusTarget::SubmitAction(SubmitAction::Cancel)) => {
            send_question_rejection(&q, String::new(), command_tx).await;
            false
        }
        _ => {
            state.ui.restore_prompt(PanePromptState::Question(q));
            true
        }
    }
}

async fn send_question_rejection(
    q: &crate::state::QuestionPromptState,
    feedback: String,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    let _ = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: q.request_id.clone(),
            approved: false,
            always_allow: false,
            feedback: (!feedback.is_empty()).then_some(feedback),
            updated_input: None,
            resolution_detail: None,
            permission_updates: vec![],
            content_blocks: None,
        })
        .await;
}

pub(crate) fn question_nav(q: &mut crate::state::QuestionPromptState, delta: i32) {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionPage;
    use crate::state::SubmitAction;

    match q.current_question {
        QuestionPage::Question(_) => q.cycle_focus(delta),
        QuestionPage::Submit => {
            let current = match q.focus_target {
                QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers) => 0,
                QuestionFocusTarget::SubmitAction(SubmitAction::Cancel) => 1,
                _ => 0,
            };
            let next = (current + delta).clamp(0, 1);
            q.focus_target = if next == 0 {
                QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers)
            } else {
                QuestionFocusTarget::SubmitAction(SubmitAction::Cancel)
            };
        }
    }
}

/// Cycle the focus within the Question state (Tab / Shift+Tab).
pub(crate) fn question_cycle_focus(state: &mut AppState, delta: i32) {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return;
    };
    q.cycle_focus(delta);
}

/// Switch the focused nav-strip tab by `delta` (Left → -1, Right → +1),
/// wrapping over the questions PLUS the trailing Submit tab.
pub(crate) fn question_switch_question(state: &mut AppState, delta: i32) {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return;
    };
    q.switch_page(delta);
}

/// Move the option cursor to the `digit`-th option (1-based) when a question is
/// focused. Out-of-range digits are no-ops.
pub(crate) fn question_select_digit(state: &mut AppState, digit: i32) {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionPage;
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return;
    };
    let QuestionPage::Question(qi_idx) = q.current_question else {
        return;
    };
    let Some(qi) = q.questions.get_mut(qi_idx) else {
        return;
    };
    let Some(idx) = digit.checked_sub(1).map(|d| d as usize) else {
        return;
    };
    if idx < qi.options.len() {
        q.focus_target = QuestionFocusTarget::QuestionOption(idx);
        q.sync_other_focus();
    }
}

pub(crate) async fn question_select_digit_and_confirm(
    state: &mut AppState,
    digit: i32,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionFooterAction;
    use crate::state::QuestionPage;

    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    let QuestionPage::Question(qi_idx) = q.current_question else {
        return false;
    };
    let Some(qi) = q.questions.get(qi_idx) else {
        return false;
    };
    if qi.is_editing() {
        return false;
    }
    let Some(idx) = digit.checked_sub(1).map(|d| d as usize) else {
        return true;
    };
    let target = if idx < qi.options.len() {
        QuestionFocusTarget::QuestionOption(idx)
    } else if idx == qi.options.len() {
        QuestionFocusTarget::OtherInput
    } else if idx == qi.options.len() + 1 {
        QuestionFocusTarget::QuestionFooter(QuestionFooterAction::ChatAboutThis)
    } else if q.is_in_plan_mode && idx == qi.options.len() + 2 {
        QuestionFocusTarget::QuestionFooter(QuestionFooterAction::SkipInterview)
    } else {
        return true;
    };
    q.focus_target = target;
    q.sync_other_focus();
    if matches!(target, QuestionFocusTarget::OtherInput) {
        return true;
    }

    let Some(PanePromptState::Question(q)) = state.ui.take_prompt() else {
        return true;
    };
    let keep_open = confirm_question_prompt(state, q, command_tx).await;
    if !keep_open {
        state.ui.finish_taken_prompt();
    }
    true
}

/// Toggle the focused option's checked state in a multi-select question
/// (Space). Single-select and footer focus are no-ops.
pub(crate) fn question_toggle_checked(state: &mut AppState) {
    use crate::state::QuestionFocusTarget;
    use crate::state::QuestionPage;
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return;
    };
    let QuestionPage::Question(qi_idx) = q.current_question else {
        return;
    };
    let Some(qi) = q.questions.get_mut(qi_idx) else {
        return;
    };
    if !qi.multi_select {
        return;
    }
    let target = match q.focus_target {
        QuestionFocusTarget::QuestionOption(idx) => idx,
        _ => return,
    };
    if let Some(pos) = qi.checked.iter().position(|i| *i == target) {
        qi.checked.swap_remove(pos);
    } else {
        qi.checked.push(target);
    }
}

/// Append a typed character into the focused question's free-text input.
/// Returns `true` if the char was consumed.
/// Caller should fall back to the normal filter-input path when this
/// returns `false`.
pub(crate) fn question_free_text_input(state: &mut AppState, c: char) -> bool {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    let Some(qi) = q.current_question_item_mut() else {
        return false;
    };
    if !qi.is_editing() {
        return false;
    }
    qi.other_input.value.push(c);
    qi.other_input.committed = false;
    true
}

/// Append pasted (or IME-committed) text into the focused question's free-text
/// input. Some terminals deliver IME-composed CJK as a bracketed paste rather
/// than per-key `Char` events, so without this the text would land in the
/// hidden background composer. Returns `true` if the paste was consumed.
pub(crate) fn question_free_text_paste(state: &mut AppState, text: &str) -> bool {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    let Some(qi) = q.current_question_item_mut() else {
        return false;
    };
    if !qi.is_editing() {
        return false;
    }
    // Paste is single-line for the free-text field; strip newlines so a
    // multi-line clipboard doesn't break the prompt layout.
    qi.other_input
        .value
        .extend(text.chars().filter(|c| *c != '\n' && *c != '\r'));
    qi.other_input.committed = false;
    true
}

/// Backspace in the focused question's free-text input. Returns `true` if the
/// keystroke was consumed.
pub(crate) fn question_free_text_backspace(state: &mut AppState) -> bool {
    let Some(PanePromptState::Question(q)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    let Some(qi) = q.current_question_item_mut() else {
        return false;
    };
    if !qi.is_editing() {
        return false;
    }
    qi.other_input.value.pop();
    qi.other_input.committed = false;
    true
}

/// Build the `{...original_input, answers, annotations}` payload shipped
/// via `UserCommand::ApprovalResponse.updated_input`.
/// Submit all collected answers (Enter on the Submit tab, or on the sole
/// question of a single-question prompt). Splices the payload into
/// `updated_input` so the tool's `execute` sees the user's choices.
async fn submit_question_answers(
    q: &crate::state::QuestionPromptState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    let updated_input = build_answer_payload(q);
    let _ = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: q.request_id.clone(),
            approved: true,
            always_allow: false,
            feedback: None,
            updated_input: Some(updated_input),
            resolution_detail: None,
            permission_updates: vec![],
            content_blocks: None,
        })
        .await;
}

pub(crate) fn build_answer_payload(q: &crate::state::QuestionPromptState) -> serde_json::Value {
    let mut answers = serde_json::Map::new();
    let mut annotations = serde_json::Map::new();

    for qi in &q.questions {
        // Multi-select submits exactly what is checked (possibly nothing —
        // the selected array is submitted verbatim, with no coercion to
        // the cursor position). Single-select submits only a committed choice.
        let chosen_indices: Vec<usize> = if qi.multi_select {
            qi.checked.clone()
        } else {
            qi.selected.into_iter().collect()
        };
        let mut labels: Vec<String> = chosen_indices
            .iter()
            .filter_map(|i| qi.options.get(*i))
            .map(|o| o.label.clone())
            .filter(|s| !s.is_empty())
            .collect();
        let other_value = qi.other_input.value.trim();
        if qi.other_input.committed && !other_value.is_empty() {
            labels.push(other_value.to_string());
        }
        let answer = labels.join(", ");
        answers.insert(qi.question.clone(), serde_json::Value::String(answer));

        // Annotation entry — preview from the selected option. The independent
        // free-text value is part of `answers`, so it is not duplicated as a note.
        let preview = qi
            .selected
            .and_then(|idx| qi.options.get(idx))
            .and_then(|o| o.preview.as_ref());
        if preview.is_some() {
            let mut entry = serde_json::Map::new();
            if let Some(p) = preview {
                entry.insert("preview".into(), serde_json::Value::String(p.clone()));
            }
            annotations.insert(qi.question.clone(), serde_json::Value::Object(entry));
        }
    }

    let mut payload = q.original_input.as_object().cloned().unwrap_or_default();
    payload.insert("answers".into(), serde_json::Value::Object(answers));
    if !annotations.is_empty() {
        payload.insert("annotations".into(), serde_json::Value::Object(annotations));
    }
    serde_json::Value::Object(payload)
}
