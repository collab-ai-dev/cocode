//! Presentation for request-style prompts.

use ratatui::prelude::Color;
use ratatui::prelude::Line;
use ratatui::prelude::Modifier;
use ratatui::prelude::Span;
use ratatui::prelude::Style;

use crate::i18n::t;
use crate::permission_options::PermissionAction;
use crate::state::ExplainerFetch;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::QuestionFocusTarget;
use crate::state::QuestionFooterAction;
use crate::state::QuestionPage;
use crate::state::QuestionPromptState;
use crate::state::RiskLevel;
use crate::state::SubmitAction;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::ActionRow;
use coco_tui_ui::widgets::ChoiceRow;
use coco_tui_ui::widgets::InputRow;
use coco_tui_ui::widgets::NavTab;
use coco_tui_ui::widgets::QuestionHeader;
use coco_tui_ui::widgets::QuestionNav;
use coco_tui_ui::widgets::QuestionRow;
use coco_tui_ui::widgets::QuestionView;
use coco_tui_ui::widgets::RowMark;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::SubmitNavTab;
use coco_tui_ui::widgets::render_select_list;

pub(crate) fn permission_content(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let is_exit_plan = matches!(p.detail, PermissionDetail::ExitPlanMode { .. });
    let exit_plan_has_plan = is_exit_plan && exit_plan_has_implementation_plan(p);
    let detail = if is_exit_plan {
        String::new()
    } else {
        permission_detail_for_prompt(p)
    };
    let risk_badge = match p.risk_level {
        Some(RiskLevel::Low) => t!("dialog.risk_low").to_string(),
        Some(RiskLevel::Medium) => t!("dialog.risk_medium").to_string(),
        Some(RiskLevel::High) => t!("dialog.risk_high").to_string(),
        None => String::new(),
    };
    // Cross-process teammate requests carry a worker badge — suffix the
    // title with `· @name` so the leader sees who is asking. The text
    // surface is monochrome; the badge color is carried on the event for
    // styled / SDK consumers.
    let worker_suffix = p
        .worker_badge
        .as_ref()
        .map(|b| format!(" · @{}", b.name))
        .unwrap_or_default();
    let title = if is_exit_plan {
        format!(
            " {}{worker_suffix} ",
            if exit_plan_has_plan {
                t!("dialog.ready_to_code")
            } else {
                t!("dialog.exit_plan_mode")
            }
        )
    } else if risk_badge.is_empty() {
        format!(" {}{worker_suffix} ", p.tool_name)
    } else {
        format!(" {}{risk_badge}{worker_suffix}", p.tool_name)
    };

    let classifier_line = if let Some(rule) = &p.classifier_auto_approved {
        t!("chat.auto_approved", rule = rule.as_str()).to_string()
    } else if p.classifier_checking {
        format!("\n{}", t!("dialog.checking"))
    } else {
        String::new()
    };

    let actions = if let Some(choices) = &p.choices {
        let selected = p.selected_choice.min(choices.len().saturating_sub(1));
        let mut lines = String::new();
        for (idx, choice) in choices.iter().enumerate() {
            let marker = if idx == selected { "▸ " } else { "  " };
            lines.push_str(&format!("{marker}{}\n", choice.label));
            if let Some(desc) = &choice.description {
                lines.push_str(&format!("    {desc}\n"));
            }
        }
        lines.push_str(t!("dialog.hints_nav_select").as_ref());
        lines
    } else {
        classic_permission_actions(p, current_mode)
    };

    // Lazy Ctrl+E risk-explainer panel: only rendered when toggled open, so
    // the default prompt body is unchanged. Risk is shown here, not in the title.
    let explainer_panel = if p.explanation_visible {
        match &p.explanation {
            ExplainerFetch::Loading => format!("\n\n{}", t!("dialog.explainer_loading")),
            ExplainerFetch::Ready(e) => {
                let level = match e.risk_level {
                    coco_types::RiskLevel::Low => t!("dialog.explainer_risk_low"),
                    coco_types::RiskLevel::Medium => t!("dialog.explainer_risk_medium"),
                    coco_types::RiskLevel::High => t!("dialog.explainer_risk_high"),
                };
                format!("\n\n{level} — {}\n{}", e.risk, e.explanation)
            }
            ExplainerFetch::Unavailable => format!("\n\n{}", t!("dialog.explainer_unavailable")),
            ExplainerFetch::NotFetched => String::new(),
        }
    } else {
        String::new()
    };

    // Border reflects the fetched risk while the panel is open; otherwise the
    // (legacy, currently always-None) `risk_level` field drives it.
    let border = match (p.explanation_visible, &p.explanation) {
        (true, ExplainerFetch::Ready(e)) if e.risk_level == coco_types::RiskLevel::High => {
            styles.error()
        }
        (true, ExplainerFetch::Ready(_)) => styles.warning(),
        _ => match p.risk_level {
            Some(RiskLevel::High) => styles.error(),
            _ => styles.warning(),
        },
    };

    let body = if is_exit_plan {
        format!("{classifier_line}\n\n{actions}{explainer_panel}")
            .trim_start()
            .to_string()
    } else {
        format!(
            "{}{classifier_line}\n\n{detail}\n\n{actions}{explainer_panel}",
            p.description
        )
    };

    (title, body, border)
}

pub(crate) fn permission_styled_content(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    styles: UiStyles<'_>,
) -> (String, Vec<Line<'static>>, Color) {
    let (title, body, border) = permission_content(p, current_mode, styles);
    let selected_style = Style::default()
        .fg(styles.selection_fg())
        .bg(styles.selection_bg())
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(styles.text());
    let dim_style = Style::default().fg(styles.dim());
    let lines = body
        .lines()
        .map(|line| {
            // Style off our own rendered markers, never substrings that could
            // appear in tool input: `▸ ` flags the selected row, a 4-space
            // indent flags choice descriptions, and the `↑/↓` glyph flags the
            // nav/shortcut hint line (a path like `v1-api` must not dim).
            let style = if line.starts_with("▸ ") {
                selected_style
            } else if line.starts_with("    ") || line.contains("↑/↓") {
                dim_style
            } else {
                text_style
            };
            Line::from(Span::styled(line.to_string(), style))
        })
        .collect();
    (title, lines, border)
}

/// Compact bottom-prompt projection for `ExitPlanMode`.
///
/// The plan body itself is rendered into the live transcript area by
/// [`exit_plan_pending_history_lines`]. Keeping this prompt to just the
/// decision rows mirrors codex-rs' "plan in history, actions in bottom pane"
/// layout and keeps the choices visible for long plans.
pub(crate) fn exit_plan_prompt_lines(
    p: &PermissionPromptState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> Vec<Line<'static>> {
    let has_plan = exit_plan_has_implementation_plan(p);
    let worker_suffix = p
        .worker_badge
        .as_ref()
        .map(|b| format!(" · @{}", b.name))
        .unwrap_or_default();
    // The "Ready to code?" title now leads the plan block in the transcript
    // (`exit_plan_pending_history_lines`); the bottom prompt keeps just the
    // proceed cue + decision rows so long plans don't push the choices off
    // screen. The worker badge rides the proceed line so cross-process
    // teammate requests still show who is asking.
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{}{}",
                if has_plan {
                    t!("dialog.plan_ready_prompt")
                } else {
                    t!("dialog.no_plan_ready_prompt")
                },
                worker_suffix
            ),
            Style::default().fg(styles.text()),
        )),
        Line::from(""),
    ];

    if let Some(choices) = &p.choices {
        let items: Vec<SelectItem> = choices
            .iter()
            .map(|choice| {
                let mut item = SelectItem::new(choice.label.clone());
                if let Some(description) = &choice.description {
                    item = item.with_secondary(description.clone());
                }
                item
            })
            .collect();
        lines.extend(render_select_list(
            &items,
            p.selected_choice,
            &SelectListStyle {
                numbered: false,
                visible_count: list_budget.max(1),
            },
            styles,
        ));
    };

    lines.push(Line::from(Span::styled(
        t!("dialog.hints_nav_select").to_string(),
        Style::default().fg(styles.dim()),
    )));
    lines
}

/// Temporary source-backed display of the pending plan while the exit-plan
/// prompt is waiting for a choice. This deliberately looks like normal
/// transcript output rather than a centered modal.
pub(crate) fn exit_plan_pending_history_lines(
    p: &PermissionPromptState,
    width: u16,
    syntax: coco_tui_ui::display::SyntaxHighlighting,
    styles: UiStyles<'_>,
) -> Vec<Line<'static>> {
    let PermissionDetail::ExitPlanMode {
        outcome,
        plan,
        plan_file_path,
        allowed_prompts,
    } = &p.detail
    else {
        return Vec::new();
    };

    if *outcome == coco_types::ExitPlanModeOutcome::NoImplementationPlan {
        return vec![Line::from(vec![
            Span::styled("• ", Style::default().fg(styles.dim())),
            Span::styled(
                t!("dialog.no_plan_heading").to_string(),
                Style::default()
                    .fg(styles.plan())
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
    };

    let plan_body = plan.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let Some(body) = plan_body else {
        return vec![Line::from(vec![
            Span::styled("• ", Style::default().fg(styles.dim())),
            Span::styled(
                t!("dialog.no_plan_heading").to_string(),
                Style::default()
                    .fg(styles.plan())
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
    };

    // TS-style "title in front": lead with the bold "Ready to code?" header,
    // then the plan heading, then the plan body. The decision rows live in
    // the bottom prompt (`exit_plan_prompt_lines`), which no longer repeats
    // the title.
    let mut lines = vec![
        Line::from(vec![
            Span::styled("• ", Style::default().fg(styles.dim())),
            Span::styled(
                t!("dialog.ready_to_code").to_string(),
                Style::default()
                    .fg(styles.plan())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            t!("dialog.plan_heading").to_string(),
            Style::default().fg(styles.plan()),
        )),
        Line::from(""),
    ];
    lines.extend(crate::presentation::plan::render_plan_markdown(
        body, styles, width, syntax,
    ));

    if let Some(path) = plan_file_path.as_deref().filter(|p| !p.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            t!("dialog.plan_file", path = path).to_string(),
            Style::default().fg(styles.dim()),
        )));
    }
    if !allowed_prompts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            t!("dialog.plan_requested_permissions").to_string(),
            Style::default().fg(styles.dim()),
        )));
        for prompt in allowed_prompts {
            lines.push(Line::from(Span::styled(
                format!("  - {}: {}", prompt.tool, prompt.prompt),
                Style::default().fg(styles.dim()),
            )));
        }
    }

    lines
}

fn exit_plan_has_implementation_plan(p: &PermissionPromptState) -> bool {
    let PermissionDetail::ExitPlanMode { outcome, .. } = &p.detail else {
        return false;
    };
    outcome.has_implementation_plan()
}

fn classic_permission_actions(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
) -> String {
    let actions = crate::permission_options::classic_actions(p, current_mode);
    let selected = p.selected_choice.min(actions.len().saturating_sub(1));
    let mut lines = format!("{}:\n", t!("dialog.actions_heading"));
    let has_local = actions.contains(&PermissionAction::AllowLocal);
    let mut shortcut_letters = Vec::new();
    for (idx, action) in actions.iter().copied().enumerate() {
        let marker = if idx == selected { "▸ " } else { "  " };
        let suffix = if idx == selected { " ◂" } else { "" };
        // Every row shows its direct-commit hotkeys (digit + letter) so the
        // mapping is visible: `y`/`a`/`n` commit their OWN row, not the
        // highlighted one (Enter commits the highlight).
        let (letter, mut label) = match action {
            PermissionAction::ApproveOnce => ("y", t!("dialog.action_approve_once").to_string()),
            PermissionAction::AllowSession => (
                if has_local { "s" } else { "a" },
                t!("dialog.action_allow_session", tool = p.tool_name.as_str()).to_string(),
            ),
            PermissionAction::AllowLocal => (
                "a",
                t!("dialog.action_allow_local", tool = p.tool_name.as_str()).to_string(),
            ),
            PermissionAction::Deny => ("n", t!("dialog.action_deny").to_string()),
        };
        // Shell tools: append the editable rule prefix to the allow rows. The
        // focused row shows a cursor and accepts typed edits.
        if matches!(
            action,
            PermissionAction::AllowSession | PermissionAction::AllowLocal
        ) && let Some(input) = p.prefix_input.as_ref()
        {
            let shown = render_prefix_with_cursor(&input.value, input.cursor, idx == selected);
            label = format!("{label}: {shown}");
        }
        shortcut_letters.push(letter.to_ascii_uppercase());
        lines.push_str(&format!("{marker}{}/{letter} · {label}{suffix}\n", idx + 1));
    }
    lines.push_str(t!("dialog.hints_nav_select").as_ref());
    lines.push_str("  ");
    // An editable allow row is focused → the shortcut letters become text, so
    // show the edit hint instead of the y/a/s/n shortcut list.
    if p.prefix_input.is_some()
        && matches!(
            crate::permission_options::classic_action_at(p, current_mode, selected),
            PermissionAction::AllowSession | PermissionAction::AllowLocal
        )
    {
        lines.push_str(t!("dialog.hints_prefix_edit").as_ref());
    } else {
        lines.push_str(
            t!(
                "dialog.hints_permission_shortcuts",
                count = actions.len().to_string(),
                shortcuts = shortcut_letters.join("/")
            )
            .as_ref(),
        );
    }
    lines
}

/// Render the editable prefix value, inserting a thin cursor glyph at the
/// cursor byte offset when the row is focused (so the user sees where typing
/// lands). Unfocused rows show the value as-is.
fn render_prefix_with_cursor(value: &str, cursor: usize, focused: bool) -> String {
    if !focused {
        return value.to_string();
    }
    let at = cursor.min(value.len());
    let (before, after) = value.split_at(at);
    format!("{before}▏{after}")
}

/// Project the domain [`QuestionPromptState`] into the pure, area-based
/// [`QuestionView`] rendered by `coco_tui_ui::widgets::QuestionWidget`. All
/// i18n + chip truncation + free-text input logic lives here so the widget crate
/// stays domain-free. Replaces the former flat-`String` `question_content`.
pub(crate) fn project_question(q: &QuestionPromptState) -> QuestionView {
    let title = t!("dialog.title_question").to_string();
    let total = q.questions.len();

    if q.questions.is_empty() {
        return QuestionView {
            header: QuestionHeader {
                title,
                chip: None,
                nav: None,
            },
            body: String::new(),
            rows: Vec::new(),
            submit_review: None,
            preview: None,
            footer_actions: Vec::new(),
            hints: t!("dialog.hints_nav_select").to_string(),
        };
    }

    let on_submit = matches!(q.current_question, QuestionPage::Submit);
    let focused_q_idx = match q.current_question {
        QuestionPage::Question(i) => i.min(total - 1),
        QuestionPage::Submit => total - 1,
    };
    let qi = &q.questions[focused_q_idx];

    let (body, rows, preview, submit_review, footer) = if on_submit {
        let rows = vec![
            QuestionRow::Action(ActionRow {
                number: 1,
                label: "Submit answers".to_string(),
                focused: q.focus_target
                    == QuestionFocusTarget::SubmitAction(SubmitAction::SubmitAnswers),
            }),
            QuestionRow::Action(ActionRow {
                number: 2,
                label: "Cancel".to_string(),
                focused: q.focus_target == QuestionFocusTarget::SubmitAction(SubmitAction::Cancel),
            }),
        ];
        (
            String::new(),
            rows,
            None,
            Some(submit_review_text(q)),
            Vec::new(),
        )
    } else {
        let selected_idx = qi.selected.filter(|idx| *idx < qi.options.len());
        let mut rows = qi
            .options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let focused = q.focus_target == QuestionFocusTarget::QuestionOption(i);
                let mark = if qi.multi_select {
                    RowMark::Check {
                        checked: qi.checked.contains(&i),
                        focused,
                    }
                } else {
                    RowMark::Radio {
                        selected: Some(i) == selected_idx,
                        focused,
                    }
                };
                QuestionRow::Choice(ChoiceRow {
                    number: i + 1,
                    label: opt.label.clone(),
                    description: opt.description.clone(),
                    mark,
                })
            })
            .collect::<Vec<_>>();
        rows.push(QuestionRow::Input(InputRow {
            number: qi.options.len() + 1,
            label: "Type something.".to_string(),
            value: qi.other_input.value.clone(),
            selected: qi.other_input.committed && !qi.other_input.value.trim().is_empty(),
            focused: q.focus_target == QuestionFocusTarget::OtherInput,
        }));

        let focused_opt = match q.focus_target {
            QuestionFocusTarget::QuestionOption(idx) => qi.options.get(idx),
            QuestionFocusTarget::OtherInput
            | QuestionFocusTarget::QuestionFooter(_)
            | QuestionFocusTarget::SubmitAction(_) => {
                selected_idx.and_then(|idx| qi.options.get(idx))
            }
        };
        let mut footer = vec![ActionRow {
            number: qi.options.len() + 2,
            label: "Chat about this".to_string(),
            focused: q.focus_target
                == QuestionFocusTarget::QuestionFooter(QuestionFooterAction::ChatAboutThis),
        }];
        if q.is_in_plan_mode {
            footer.push(ActionRow {
                number: qi.options.len() + 3,
                label: "Skip interview and plan immediately".to_string(),
                focused: q.focus_target
                    == QuestionFocusTarget::QuestionFooter(QuestionFooterAction::SkipInterview),
            });
        }
        (
            qi.question.clone(),
            rows,
            focused_opt.and_then(|o| o.preview.clone()),
            None,
            footer,
        )
    };

    let hints = if on_submit {
        "Enter to select · Tab/Arrow keys to navigate · Esc to cancel".to_string()
    } else {
        let mut hints = String::from(
            "Enter to select · Tab/Arrow keys to navigate · ctrl+g to edit in Vim · Esc to cancel",
        );
        if qi.multi_select {
            hints.push_str(" · Space to toggle");
        }
        hints
    };

    // >1 question → a nav strip (every header + the trailing Submit tab, current
    // highlighted) replaces the single-question chip; 1 question keeps `[chip]`.
    let nav = (total > 1).then(|| QuestionNav {
        tabs: q
            .questions
            .iter()
            .map(|item| NavTab {
                header: chip(&item.header),
                answered: q.question_has_answer(item),
            })
            .collect(),
        current: focused_q_idx,
        submit: Some(SubmitNavTab {
            focused: on_submit,
            ready: q.all_answered(),
        }),
    });

    QuestionView {
        header: QuestionHeader {
            title,
            chip: (total == 1 && !qi.header.is_empty()).then(|| chip(&qi.header)),
            nav,
        },
        body,
        rows,
        submit_review,
        preview,
        footer_actions: footer,
        hints,
    }
}

/// Read-only "Review your answers" body shown above the Submit/Cancel list when
/// the Submit tab is focused. Shows a warning when not all questions are
/// answered, then `● question → answer` per row, then the "Ready to submit?" prompt.
fn submit_review_text(q: &QuestionPromptState) -> String {
    let mut out = String::from("Review your answers");
    if !q.all_answered() {
        out.push_str("\n\n⚠ You have not answered all questions");
    }
    for item in &q.questions {
        let answer = q.committed_answer_for(item);
        let answer = answer.trim();
        let answer = if answer.is_empty() {
            "(unanswered)"
        } else {
            answer
        };
        out.push_str(&format!("\n\n● {}\n   → {answer}", item.question));
    }
    out.push_str("\n\nReady to submit your answers?");
    out
}

/// Loose upper bound (unwrapped line count) of a permission prompt's
/// scrollable detail body, used to bound the scroll offset in the update
/// handler. The render pass applies the exact clamp against the wrapped height.
pub(crate) fn permission_body_line_count(p: &PermissionPromptState) -> usize {
    permission_detail_for_prompt(p).lines().count()
}

fn permission_detail_for_prompt(p: &PermissionPromptState) -> String {
    if matches!(p.detail, PermissionDetail::Generic { .. }) {
        return p.display_input.as_display_str().to_string();
    }
    permission_detail(&p.detail)
}

fn permission_detail(detail: &PermissionDetail) -> String {
    match detail {
        PermissionDetail::Bash {
            command,
            risk_description,
            working_dir,
        } => shell_detail(
            t!("dialog.perm_command", command = command.as_str()).to_string(),
            risk_description,
            working_dir,
        ),
        PermissionDetail::FileEdit { path, diff } => {
            let preview = truncate_preview(diff, 500);
            format!(
                "{}\n\n{preview}",
                t!("dialog.perm_file", path = path.as_str())
            )
        }
        PermissionDetail::FileWrite {
            path,
            content_preview,
            is_new_file,
        } => {
            let action = if *is_new_file {
                t!("dialog.perm_create")
            } else {
                t!("dialog.perm_overwrite")
            };
            let file_line = t!("dialog.perm_file", path = path.as_str());
            let action_label = t!("dialog.perm_action_label");
            format!("{action_label}: {action}\n{file_line}\n\n{content_preview}")
        }
        PermissionDetail::Filesystem { operation, path } => t!(
            "dialog.perm_filesystem",
            operation = operation.as_str(),
            path = path.as_str()
        )
        .to_string(),
        PermissionDetail::WebFetch { url, method } => t!(
            "dialog.perm_web",
            method = method.as_str(),
            url = url.as_str()
        )
        .to_string(),
        PermissionDetail::Skill {
            skill_name,
            skill_description,
        } => {
            let desc = skill_description.as_deref().unwrap_or("");
            format!(
                "{}\n{desc}",
                t!("dialog.perm_skill", name = skill_name.as_str())
            )
        }
        PermissionDetail::SedEdit {
            path,
            pattern,
            replacement,
        } => t!(
            "dialog.perm_sed",
            path = path.as_str(),
            pattern = pattern.as_str(),
            replacement = replacement.as_str()
        )
        .to_string(),
        PermissionDetail::NotebookEdit {
            path,
            cell_id,
            change_preview,
        } => format!(
            "{}\n\n{change_preview}",
            t!(
                "dialog.perm_notebook",
                path = path.as_str(),
                cell = cell_id.as_str()
            )
        ),
        PermissionDetail::McpTool {
            server_name,
            tool_name,
            input_preview,
        } => format!(
            "{}\n\n{input_preview}",
            t!(
                "dialog.perm_mcp",
                server = server_name.as_str(),
                tool = tool_name.as_str()
            )
        ),
        PermissionDetail::PowerShell {
            command,
            risk_description,
            working_dir,
        } => shell_detail(
            t!("dialog.perm_powershell", command = command.as_str()).to_string(),
            risk_description,
            working_dir,
        ),
        PermissionDetail::ComputerUse {
            action,
            description,
        } => format!(
            "{}\n\n{description}",
            t!("dialog.perm_computer_use", action = action.as_str())
        ),
        PermissionDetail::ExitPlanMode { .. } => String::new(),
        PermissionDetail::Generic { input_preview } => input_preview.clone(),
    }
}

fn shell_detail(
    mut detail: String,
    risk_description: &Option<String>,
    working_dir: &Option<String>,
) -> String {
    if let Some(dir) = working_dir {
        detail.push_str("\n\n");
        detail.push_str(&t!("dialog.perm_directory", path = dir.as_str()));
    }
    if let Some(risk) = risk_description {
        detail.push_str("\n\n");
        detail.push_str(&t!("dialog.perm_risk_note", risk = risk.as_str()));
    }
    detail
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx == max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

/// Max display width of a question's `header` chip: 12 characters.
const CHIP_MAX_CHARS: usize = 12;

fn chip(s: &str) -> String {
    if s.chars().count() > CHIP_MAX_CHARS {
        let truncated: String = s.chars().take(CHIP_MAX_CHARS - 1).collect();
        format!("{truncated}…")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
#[path = "request.test.rs"]
mod tests;
