//! AskUserQuestion prompt state — pages, focus targets, option/other-input
//! navigation, and answer assembly.
//!
//! Split from `surface_payloads.rs` (the Stage-3 "one migrated surface = one
//! state file" pattern, like `agents_dialog.rs`). The behavior layer lives in
//! `bottom_pane/question.rs`; renderers read this via
//! `presentation/request.rs`.

/// Question state (AskUserQuestion tool).
#[derive(Debug, Clone)]
pub struct QuestionPromptState {
    pub request_id: String,
    /// Original tool input dict, stored verbatim so the answer payload
    /// can re-emit fields the model supplied that the TUI doesn't render
    /// (e.g. `metadata.source`). Stored AND re-emitted because the
    /// splice protocol in `update/state.rs` rebuilds the input as
    /// `{...original_input, answers, annotations}` — dropping the
    /// `original_input` spread would silently strip those fields.
    pub original_input: serde_json::Value,
    pub questions: Vec<QuestionItem>,
    /// Currently visible question page, or the multi-question submit page.
    pub current_question: QuestionPage,
    /// Focus within the currently visible page. This never encodes which page
    /// is visible, so footer/submit focus can no longer fall back to question 0.
    pub focus_target: QuestionFocusTarget,
    /// Plan-mode gate for the Skip-interview footer item. Set from
    /// `state.session.permission_mode == PermissionMode::Plan` when the
    /// state is constructed.
    pub is_in_plan_mode: bool,
}

/// Page shown by the AskUserQuestion prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionPage {
    Question(usize),
    Submit,
}

/// What the user is focused on inside the active question page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionFocusTarget {
    QuestionOption(usize),
    OtherInput,
    QuestionFooter(QuestionFooterAction),
    SubmitAction(SubmitAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionFooterAction {
    ChatAboutThis,
    SkipInterview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitAction {
    SubmitAnswers,
    Cancel,
}

/// One question in the AskUserQuestion state.
#[derive(Debug, Clone)]
pub struct QuestionItem {
    /// Short label rendered as a chip — e.g. "Auth method".
    pub header: String,
    /// Full question text — typically ends with "?".
    pub question: String,
    pub options: Vec<QuestionOption>,
    /// `true` allows checkbox-style multi-selection, `false` is radio.
    pub multi_select: bool,
    /// Committed option index for radio-style questions. `None` means the user
    /// has focused a row but has not pressed Enter / a number shortcut yet.
    pub selected: Option<usize>,
    /// Indices toggled on for multi-select. Empty in single-select mode
    /// (the Enter handler then falls back to `selected`).
    pub checked: Vec<usize>,
    /// Free-form answer state rendered as the final `Type something.` row.
    pub other_input: OtherInputState,
}

impl QuestionItem {
    /// True when typed characters should edit [`Self::other_input`].
    pub fn is_editing(&self) -> bool {
        self.other_input.focused
    }
}

#[derive(Debug, Clone, Default)]
pub struct OtherInputState {
    pub focused: bool,
    pub value: String,
    pub committed: bool,
}

/// One choice within a [`QuestionItem`].
#[derive(Debug, Clone)]
pub struct QuestionOption {
    /// 1-5 word label shown in the option list.
    pub label: String,
    /// Longer explanation rendered under the label.
    pub description: String,
    /// Optional preview content (Markdown / monospace) shown side-by-side
    /// when this option is focused. `None` for plain options.
    pub preview: Option<String>,
}

impl QuestionPromptState {
    pub(crate) fn current_question_index(&self) -> Option<usize> {
        let QuestionPage::Question(idx) = self.current_question else {
            return None;
        };
        (idx < self.questions.len()).then_some(idx)
    }

    pub(crate) fn current_question_item(&self) -> Option<&QuestionItem> {
        self.current_question_index()
            .and_then(|idx| self.questions.get(idx))
    }

    pub(crate) fn current_question_item_mut(&mut self) -> Option<&mut QuestionItem> {
        let idx = self.current_question_index()?;
        self.questions.get_mut(idx)
    }

    pub(crate) fn set_question_page(&mut self, idx: usize) {
        if self.questions.is_empty() {
            return;
        }
        let idx = idx.min(self.questions.len() - 1);
        self.current_question = QuestionPage::Question(idx);
        self.focus_target = if self.questions[idx].options.is_empty() {
            QuestionFocusTarget::OtherInput
        } else {
            QuestionFocusTarget::QuestionOption(
                self.questions[idx]
                    .selected
                    .unwrap_or(0)
                    .min(self.questions[idx].options.len().saturating_sub(1)),
            )
        };
        self.sync_other_focus();
    }

    pub(crate) fn set_submit_page(&mut self) {
        self.current_question = QuestionPage::Submit;
        self.focus_target = QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers);
        self.sync_other_focus();
    }

    pub(crate) fn advance_question_or_submit(&mut self) -> bool {
        let Some(idx) = self.current_question_index() else {
            return false;
        };
        if idx + 1 < self.questions.len() {
            self.set_question_page(idx + 1);
            return true;
        }
        if self.questions.len() > 1 {
            self.set_submit_page();
            return true;
        }
        false
    }

    pub(crate) fn switch_page(&mut self, delta: i32) {
        if self.questions.len() <= 1 {
            return;
        }
        let len = self.questions.len() + 1;
        let current = match self.current_question {
            QuestionPage::Question(idx) => idx.min(self.questions.len() - 1),
            QuestionPage::Submit => self.questions.len(),
        };
        let next = (current as i32 + delta).rem_euclid(len as i32) as usize;
        if next == self.questions.len() {
            self.set_submit_page();
        } else {
            self.set_question_page(next);
        }
    }

    pub(crate) fn cycle_focus(&mut self, delta: i32) {
        use QuestionFocusTarget as Target;
        use QuestionFooterAction as Footer;
        use QuestionPage as Page;
        use SubmitAction as Action;

        let order = match self.current_question {
            Page::Question(idx) => {
                let Some(item) = self.questions.get(idx) else {
                    return;
                };
                let mut order: Vec<Target> = (0..item.options.len())
                    .map(Target::QuestionOption)
                    .collect();
                order.push(Target::OtherInput);
                order.push(Target::QuestionFooter(Footer::ChatAboutThis));
                if self.is_in_plan_mode {
                    order.push(Target::QuestionFooter(Footer::SkipInterview));
                }
                order
            }
            Page::Submit => vec![
                Target::SubmitAction(Action::SubmitAnswers),
                Target::SubmitAction(Action::Cancel),
            ],
        };
        if order.is_empty() {
            return;
        }
        let idx = order
            .iter()
            .position(|target| *target == self.focus_target)
            .unwrap_or(0) as i32;
        self.focus_target = order[(idx + delta).rem_euclid(order.len() as i32) as usize];
        self.sync_other_focus();
    }

    pub(crate) fn commit_focused_answer(&mut self) {
        let QuestionPage::Question(qidx) = self.current_question else {
            return;
        };
        let Some(item) = self.questions.get_mut(qidx) else {
            return;
        };
        match self.focus_target {
            QuestionFocusTarget::QuestionOption(oidx)
                if !item.multi_select && oidx < item.options.len() =>
            {
                item.selected = Some(oidx);
                item.other_input.committed = false;
            }
            QuestionFocusTarget::OtherInput if !item.other_input.value.trim().is_empty() => {
                item.other_input.committed = true;
                if !item.multi_select {
                    item.selected = None;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn sync_other_focus(&mut self) {
        let focused_idx = match (self.current_question, self.focus_target) {
            (QuestionPage::Question(idx), QuestionFocusTarget::OtherInput) => Some(idx),
            _ => None,
        };
        for (idx, item) in self.questions.iter_mut().enumerate() {
            item.other_input.focused = focused_idx == Some(idx);
        }
    }

    /// Build the "Chat about this" rejection-feedback prose.
    ///
    /// The leading-whitespace lines are intentional — the indented
    /// template literal and literal indentation are load-bearing.
    pub fn chat_about_this_feedback(&self) -> String {
        let questions_with_answers = self.format_questions_with_answers(/*concise=*/ false);
        format!(
            "The user wants to clarify these questions.\n    \
             This means they may have additional information, context or questions for you.\n    \
             Take their response into account and then reformulate the questions if appropriate.\n    \
             Start by asking them what they would like to clarify.\n\n    \
             Questions asked:\n{questions_with_answers}"
        )
    }

    /// Build the "Skip interview and plan immediately" rejection-feedback
    /// prose. Caller is responsible for gating on `is_in_plan_mode` —
    /// this fn is pure.
    pub fn skip_interview_feedback(&self) -> String {
        let questions_with_answers = self.format_questions_with_answers(/*concise=*/ false);
        format!(
            "The user has indicated they have provided enough answers for the plan interview.\n\
             Stop asking clarifying questions and proceed to finish the plan with the information you have.\n\n\
             Questions asked and answers provided:\n{questions_with_answers}"
        )
    }

    /// Helper used by both feedback builders — extracted to keep the
    /// prose constants the only place that diverges.
    fn format_questions_with_answers(&self, _concise: bool) -> String {
        self.questions
            .iter()
            .map(|q| {
                let answer = self.answer_for(q, /*include_uncommitted_other=*/ true);
                if answer.is_empty() {
                    format!("- \"{}\"\n  (No answer provided)", q.question)
                } else {
                    format!("- \"{}\"\n  Answer: {}", q.question, answer)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Peek the would-be answer for `q` without committing. Used by the
    /// feedback synthesizers — they show what the user partially answered
    /// before deciding to bail out via Chat-about-this / Skip-interview.
    /// Single-select picks the selected option label unless the free-text input
    /// is focused with content; multi-select joins checked labels plus typed
    /// free text when present.
    pub(crate) fn committed_answer_for(&self, q: &QuestionItem) -> String {
        self.answer_for(q, /*include_uncommitted_other=*/ false)
    }

    fn answer_for(&self, q: &QuestionItem, include_uncommitted_other: bool) -> String {
        let typed = q.other_input.value.trim();
        if include_uncommitted_other && q.other_input.focused {
            return typed.to_string();
        }
        if include_uncommitted_other && !typed.is_empty() {
            return typed.to_string();
        }
        if q.other_input.committed && !typed.is_empty() {
            return typed.to_string();
        }

        let label_for = |idx: usize| -> Option<&str> {
            let opt = q.options.get(idx)?;
            Some(opt.label.as_str())
        };
        if q.multi_select {
            let mut labels = q
                .checked
                .iter()
                .filter_map(|i| label_for(*i))
                .map(str::to_string)
                .collect::<Vec<_>>();
            if (q.other_input.committed || include_uncommitted_other) && !typed.is_empty() {
                labels.push(typed.to_string());
            }
            labels.join(", ")
        } else {
            q.selected.and_then(label_for).unwrap_or("").to_string()
        }
    }

    /// Whether `q` currently resolves to a non-empty answer. Drives the
    /// multi-question nav strip's ☒/☐ checkbox. Single-select questions
    /// pre-select the first option, so they read as answered unless "Other"
    /// is focused with an empty buffer; multi-select reads unanswered until
    /// something is checked.
    pub(crate) fn question_has_answer(&self, q: &QuestionItem) -> bool {
        !self.committed_answer_for(q).trim().is_empty()
    }

    /// True when every question resolves to an answer — drives the Submit tab's
    /// ✔/☐ marker and the "ready to submit" hint.
    pub(crate) fn all_answered(&self) -> bool {
        self.questions.iter().all(|q| self.question_has_answer(q))
    }
}

#[cfg(test)]
#[path = "question_prompt.test.rs"]
mod tests;
