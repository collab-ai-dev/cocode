//! Interactive viewport renderer for the native-scrollback surface.

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::FrameLayout;
use crate::presentation::activity::TurnActivityView;
use crate::presentation::activity::inline_activity_height;
use crate::presentation::activity::turn_activity_view;
use crate::presentation::input::InlinePopupView;
use crate::presentation::input::inline_popup_view;
use crate::presentation::layout::text_width;
use crate::state::AppState;
use crate::state::FocusTarget;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::surface::modal::SurfaceFramePlan;
use crate::surface::modal::render_modal_surface;
use crate::surface::modal::required_text_surface_height_for_box;
use crate::widgets::SuggestionPopup;
use crate::widgets::ToastWidget;
use crate::widgets::TranscriptLayoutIndex;
use coco_tui_ui::constants;
use coco_tui_ui::engine::terminal::SurfaceFrame;
use coco_tui_ui::style::UiStyles;

/// Render the retained native-scrollback viewport.
///
/// Finalized transcript messages normally live above the viewport in native
/// scrollback. Compatibility fallback mode renders them inside the retained
/// viewport instead so terminals without usable scrollback do not drop history.
pub(crate) fn render_interactive_viewport(
    frame: &mut SurfaceFrame<'_>,
    state: &AppState,
    plan: SurfaceFramePlan,
    transcript_layout: &mut TranscriptLayoutIndex,
    precomputed_live: Option<Vec<Line<'static>>>,
) -> FrameLayout {
    let mut layout = FrameLayout::default();
    let area = frame.area();
    let styles = UiStyles::new(&state.ui.theme);

    render_live_viewport(
        frame,
        area,
        state,
        styles,
        plan,
        &mut layout,
        precomputed_live,
    );

    // Read-only agent-view overlay paints over the live viewport (composer
    // included → suppressed) when a subagent's conversation is open. Drawn
    // before toasts/modals so those still layer on top.
    if state.session.viewing_agent_id.is_some() {
        crate::surface::agent_view::render_agent_view_overlay(frame, area, state, styles);
    }

    if state.ui.has_toasts() {
        render_toasts(frame, area, &state.ui.toasts, styles);
    }

    if let Some(modal) = state.ui.modal.as_ref() {
        render_modal_surface(
            frame,
            area,
            Some(layout.input),
            modal,
            state,
            transcript_layout,
            styles,
        );
    }

    layout
}

pub(crate) fn interactive_viewport_desired_height(
    state: &AppState,
    width: u16,
    max_height: u16,
    plan: SurfaceFramePlan,
    precomputed_live_height: Option<u16>,
) -> u16 {
    if width == 0 || max_height == 0 {
        return 0;
    }

    // The live tail is built once per frame in `Tui::draw` and threaded in
    // here as `precomputed_live_height`; only fall back to rebuilding it when
    // no precomputed value is supplied (alt-screen / tests).
    let live_content_height = precomputed_live_height.unwrap_or_else(|| {
        let styles = UiStyles::new(&state.ui.theme);
        build_live_tail_lines(state, styles, width, plan).len() as u16
    });
    let activity = turn_activity_view(state, width);
    let activity_rows = inline_activity_height(&activity, max_height, width);
    let queue_rows: u16 = crate::widgets::QueueStatusWidget::height(&state.session.queued_commands);
    // Mirror render_live_viewport's reservations so the sizing pass and the
    // paint pass agree on viewport height (these were previously omitted here).
    let status_indicator_rows: u16 = if show_status_indicator(state) { 1 } else { 0 };
    let background_pills_rows: u16 =
        if crate::widgets::build_background_pills_view(state).is_empty() {
            0
        } else {
            1
        };
    let bottom = interaction_pane_bottom_reservation(
        state,
        width,
        max_height,
        status_indicator_rows,
        background_pills_rows,
        activity_rows,
        queue_rows,
    );
    let prompt_rows = interaction_prompt_height(state, width, max_height);
    let input_height = bottom.input_height;
    let stash_rows = bottom.stash_rows;
    let bottom_height = bottom.bottom_height;
    let other_fixed_rows = status_indicator_rows
        + background_pills_rows
        + activity_rows
        + queue_rows
        + prompt_rows
        + input_height
        + stash_rows;
    let fixed_rows = other_fixed_rows + bottom_height;
    let desired = fixed_rows + live_content_height.min(max_height.saturating_sub(fixed_rows));
    desired.min(max_height)
}

#[derive(Debug, Clone, Copy)]
struct InteractionPaneBottomReservation {
    input_height: u16,
    stash_rows: u16,
    bottom_height: u16,
}

fn input_height_for_state(state: &AppState) -> u16 {
    if matches!(
        state.ui.interaction.active_prompt,
        Some(PanePromptState::Question(_))
    ) {
        return 0;
    }
    // Grow the composer with its hard line breaks (mirrors TS, whose TextInput
    // expands with content) so recalled multi-message edits and multi-line input
    // show on separate rows. Capped at MAX_INPUT_HEIGHT content rows (then the
    // composer scrolls). +2 for the top/bottom borders.
    let max_content = (constants::MAX_INPUT_HEIGHT as usize)
        .saturating_sub(2)
        .max(1);
    let line_count = state
        .ui
        .input
        .text()
        .split('\n')
        .count()
        .clamp(1, max_content);
    line_count as u16 + 2
}

#[allow(clippy::too_many_arguments)]
fn interaction_pane_bottom_reservation(
    state: &AppState,
    _width: u16,
    max_height: u16,
    status_indicator_rows: u16,
    background_pills_rows: u16,
    activity_rows: u16,
    queue_rows: u16,
) -> InteractionPaneBottomReservation {
    let stash_rows: u16 =
        if crate::widgets::StashNotice::should_display(state.ui.stashed_input.as_ref()) {
            1
        } else {
            0
        };
    let input_height = input_height_for_state(state);
    let inline_popup = inline_popup_view(state);
    let popup_items = inline_popup
        .as_ref()
        .map(InlinePopupView::item_count)
        .unwrap_or(0);
    let popup_active = popup_items > 0;
    let status_height: u16 = crate::status_bar::status_bar_height(state);
    let prompt_rows = interaction_prompt_height(state, _width, max_height);
    // Match render_live_viewport's avail_below_input base exactly (it subtracts
    // the status-indicator and background-pill rows too) so the sizing pass and
    // the paint pass derive the same bottom_height when a popup is active.
    let other_fixed_rows = status_indicator_rows
        + background_pills_rows
        + activity_rows
        + queue_rows
        + prompt_rows
        + input_height
        + stash_rows;
    let avail_below_input = max_height.saturating_sub(other_fixed_rows);
    let bottom_height: u16 = if popup_active {
        popup_row_budget(popup_items, avail_below_input)
    } else {
        status_height.min(avail_below_input)
    };
    InteractionPaneBottomReservation {
        input_height,
        stash_rows,
        bottom_height,
    }
}

/// The turn-status spinner ("Enchanting… · esc to interrupt") shows while
/// the turn is actively working, or while a compaction runs — but NOT while
/// the turn is paused on a blocking approval prompt. During an approval the
/// prompt itself is the UI; a spinner row wedged between the content and the
/// decision options is pure noise (TS/codex pause the turn → no spinner).
/// `turn_paused()` is the existing paused-on-permission-prompt signal.
fn show_status_indicator(state: &AppState) -> bool {
    (state.ui.ephemeral.turn_active() && !state.ui.ephemeral.turn_paused())
        || state.session.is_compacting
}

/// Input-token count for the running turn's `↑` segment in the status spinner.
///
/// This is the CURRENT turn's prompt size, not the session total: `↓` is this
/// turn's live output estimate, so `↑` must match per-turn semantics. Input
/// isn't streamed (no chars/4-style estimate), but the prompt *is* the current
/// context, so we read the live context fill (`render_context_usage().used`)
/// rather than `session.token_usage.input_tokens` — the latter is the summed
/// session total (the bottom status bar's `↑23.8K`) and would wrongly grow
/// across turns. `None` (→ `…`) when the context window is unknown.
fn live_input_tokens(state: &AppState) -> Option<i64> {
    crate::presentation::context_usage::render_context_usage(state)
        .map(|usage| usage.used)
        .filter(|used| *used > 0)
}

fn render_live_viewport(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    state: &AppState,
    styles: UiStyles<'_>,
    plan: SurfaceFramePlan,
    layout: &mut FrameLayout,
    precomputed_live: Option<Vec<Line<'static>>>,
) {
    let input_height = input_height_for_state(state);
    let activity = turn_activity_view(state, area.width);
    let activity_rows = inline_activity_height(&activity, area.height, area.width);
    let queue_rows: u16 = crate::widgets::QueueStatusWidget::height(&state.session.queued_commands);
    let stash_rows: u16 =
        if crate::widgets::StashNotice::should_display(state.ui.stashed_input.as_ref()) {
            1
        } else {
            0
        };
    let prompt_rows = interaction_prompt_height(state, area.width, area.height);

    let live_lines =
        precomputed_live.unwrap_or_else(|| build_live_tail_lines(state, styles, area.width, plan));
    let live_content_height = live_lines.len() as u16;
    let inline_popup = inline_popup_view(state);
    let popup_items = inline_popup
        .as_ref()
        .map(InlinePopupView::item_count)
        .unwrap_or(0);
    let popup_active = popup_items > 0;
    let status_height: u16 = crate::status_bar::status_bar_height(state);
    // Single-row main-turn status indicator (spinner + verb + elapsed
    // + tokens) above the activity panel — visible only while a turn
    // is running.
    let status_indicator_rows: u16 = if show_status_indicator(state) { 1 } else { 0 };
    // Background pills bar — shown when any subagent is backgrounded. The
    // agent SWITCHER is not a separate rail here; it is merged into the
    // "Agents" activity panel (a `◯ main` row + cursor when the rail holds
    // focus), so this slot stays the single-row pills bar.
    let pills_view = crate::widgets::build_background_pills_view(state);
    let background_pills_rows: u16 = if pills_view.is_empty() { 0 } else { 1 };
    let other_fixed_rows = status_indicator_rows
        + activity_rows
        + background_pills_rows
        + queue_rows
        + prompt_rows
        + input_height
        + stash_rows;
    let avail_below_input = area.height.saturating_sub(other_fixed_rows);
    let bottom_height: u16 = if popup_active {
        popup_row_budget(popup_items, avail_below_input)
    } else {
        status_height.min(avail_below_input)
    };
    let avail_for_live_tail = avail_below_input.saturating_sub(bottom_height);
    let live_tail_height = live_content_height.min(avail_for_live_tail);

    let [
        _filler,
        live_tail,
        status_indicator_area,
        activity_area,
        background_pills_area,
        queue,
        prompt,
        input,
        stash,
        bottom,
    ] = area.layout(&Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(live_tail_height),
        Constraint::Length(status_indicator_rows),
        Constraint::Length(activity_rows),
        Constraint::Length(background_pills_rows),
        Constraint::Length(queue_rows),
        Constraint::Length(prompt_rows),
        Constraint::Length(input_height),
        Constraint::Length(stash_rows),
        Constraint::Length(bottom_height),
    ]));

    layout.input = input;

    render_live_tail_lines(
        frame,
        live_tail,
        live_lines,
        live_content_height,
        avail_for_live_tail,
        state,
    );
    if status_indicator_area.height > 0 && show_status_indicator(state) {
        // Elapsed reads from the UI ephemeral helper so the displayed
        // clock subtracts paused time (permission-prompt blocks). The
        // turn-start anchor is on the running-turn record itself —
        // no Option<Instant> threading required.
        // Owned here so the borrow outlives the `StatusIndicatorView`
        // (`verb: &'a str`). Empty unless the user just hit Esc/Ctrl+C.
        let interrupting_verb = state
            .ui
            .ephemeral
            .is_interrupting()
            .then(|| crate::i18n::t!("status.interrupting").to_string());
        let view = if state.ui.ephemeral.turn_active() {
            let elapsed_ms = state.ui.ephemeral.elapsed_ms(std::time::Instant::now());
            let effort = state.session.thinking_effort;
            let effort_level = effort.is_explicit_level().then(|| effort.as_str());
            // An "in-process teammate" maps to `SubagentKind::Teammate` and
            // the "not idle" predicate is `status == Running`.
            let has_running_teammates = state.session.subagents.iter().any(|a| {
                matches!(a.kind, crate::state::SubagentKind::Teammate)
                    && matches!(a.status, crate::state::session::SubagentStatus::Running)
            });
            let teammate_tokens: i64 = state
                .session
                .subagents
                .iter()
                .filter(|a| {
                    matches!(a.kind, crate::state::SubagentKind::Teammate)
                        && matches!(a.status, crate::state::session::SubagentStatus::Running)
                })
                .map(|a| a.total_tokens)
                .sum();
            coco_tui_ui::widgets::StatusIndicatorView {
                verb: interrupting_verb
                    .as_deref()
                    .or_else(|| state.ui.ephemeral.current_verb())
                    .unwrap_or("Working"),
                elapsed_ms,
                // Input doesn't stream, so there's no live estimate (unlike
                // output). Surface the known prompt-token count from the
                // session usage — the same source the bottom status bar shows
                // as `↑23.8K` — so the spinner reads `↑23.8K ↓28` instead of
                // a perpetual `↑…`.
                input_tokens: live_input_tokens(state),
                output_tokens: state
                    .ui
                    .ephemeral
                    .live_output_tokens()
                    .saturating_add(teammate_tokens),
                effort_level,
                show_interrupt_hint: true,
                force_show_tokens: false,
                has_running_teammates,
            }
        } else {
            let elapsed_ms = state
                .session
                .compaction_started_at
                .map(|started| started.elapsed().as_millis() as i64)
                .unwrap_or(0);
            coco_tui_ui::widgets::StatusIndicatorView {
                verb: state
                    .session
                    .compaction_phase
                    .map(crate::state::session::CompactionPhaseLabel::status_label)
                    .unwrap_or("Compacting conversation"),
                elapsed_ms,
                input_tokens: None,
                output_tokens: 0,
                effort_level: None,
                show_interrupt_hint: true,
                force_show_tokens: false,
                has_running_teammates: false,
            }
        };
        frame.render_widget(
            coco_tui_ui::widgets::StatusIndicator::new(view, styles),
            status_indicator_area,
        );
    }
    if activity_area.height > 0 && matches!(&activity, TurnActivityView::Surface(_)) {
        frame.render_widget(
            crate::widgets::ActivityPanel::new(activity, styles),
            activity_area,
        );
    }
    if background_pills_area.height > 0 {
        frame.render_widget(
            crate::widgets::BackgroundPills::new(&pills_view, styles),
            background_pills_area,
        );
    }
    if queue_rows > 0 {
        frame.render_widget(
            crate::widgets::QueueStatusWidget::new(&state.session.queued_commands, styles),
            queue,
        );
    }
    if prompt_rows > 0 {
        if matches!(
            state.ui.interaction.active_prompt,
            Some(PanePromptState::Question(_))
        ) {
            layout.question_prompt = prompt;
        }
        render_interaction_prompt(frame, prompt, state, styles);
    }
    render_input(frame, state, input, styles);
    if stash_rows > 0
        && let Some(s) = state.ui.stashed_input.as_ref()
    {
        frame.render_widget(crate::widgets::StashNotice::new(s, styles), stash);
    }

    if popup_active {
        if let Some(popup_view) = inline_popup {
            let popup = crate::widgets::SuggestionPopup::new(popup_view.items, styles)
                .selected(popup_view.selected)
                .max_visible(bottom_height as usize);
            frame.render_widget(popup, bottom);
        }
    } else if bottom_height > 0 {
        frame.render_widget(
            crate::status_bar::StatusBarWidget::new(state, styles),
            bottom,
        );
    }
}

/// Rows to reserve for the active suggestion popup: as many as there are
/// items, capped at `DEFAULT_MAX_VISIBLE` and by the space below the input.
///
/// Sizing to the item count — rather than always reserving the full cap —
/// keeps the composer's vertical shift proportional to the popup (a 2-item
/// popup shifts the input ~2 rows, not 10). Mirrors codex's content-based
/// `calculate_required_height`. The widget renders exactly one row per item
/// with no border/title/hint chrome, so item count is the exact row need.
fn popup_row_budget(item_count: usize, avail_below_input: u16) -> u16 {
    let rows = item_count.min(SuggestionPopup::DEFAULT_MAX_VISIBLE as usize) as u16;
    rows.min(avail_below_input)
}

pub(crate) fn build_live_tail_lines(
    state: &AppState,
    styles: UiStyles<'_>,
    width: u16,
    plan: SurfaceFramePlan,
) -> Vec<Line<'static>> {
    // Phase 3d (§4): the renderer consumes engine cells directly.
    // Compatibility-fallback mode keeps finalized history inside the
    // viewport; otherwise the native scrollback owns it and this layer
    // renders an empty tail.
    let committed_cells: &[crate::transcript::cells::RenderedCell] =
        if plan.finalized_history_in_viewport() {
            state.session.transcript.cells()
        } else {
            &[]
        };
    let mut chat = crate::transcript::render::CellsRenderer::new(committed_cells, styles)
        .streaming(state.ui.streaming.as_ref())
        .show_thinking(state.ui.show_thinking)
        .show_system_reminders(state.ui.show_system_reminders)
        .tool_executions(&state.session.tool_executions)
        .width(width)
        .syntax_highlighting(state.ui.display_settings.syntax_highlighting)
        .cwd(state.session.working_dir.as_deref())
        .kb_handle(&state.ui.kb_handle);
    if !state.ui.collapsed_tools.is_empty() {
        chat = chat.collapsed_tools(&state.ui.collapsed_tools);
    }
    let mut lines = chat.build_lines_owned();
    if let Some(prompt) = active_exit_plan_prompt(state) {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(
            crate::presentation::request::exit_plan_pending_history_lines(
                prompt,
                width,
                state.ui.display_settings.syntax_highlighting,
                styles,
            ),
        );
    }
    lines
}

fn render_live_tail_lines(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    lines: Vec<Line<'static>>,
    content_height: u16,
    avail: u16,
    state: &AppState,
) {
    if area.height == 0 {
        return;
    }
    let scroll_offset = if content_height > avail {
        let overflow = content_height - avail;
        if state.ui.user_scrolled {
            state.ui.scroll_offset.max(0).min(overflow as i32) as u16
        } else {
            overflow
        }
    } else {
        0
    };
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));
    frame.render_widget(paragraph, area);
}

fn render_input(frame: &mut SurfaceFrame<'_>, state: &AppState, area: Rect, styles: UiStyles<'_>) {
    let is_focused = state.ui.focus == FocusTarget::Input;
    let history_search =
        state
            .ui
            .history_search
            .as_ref()
            .map(|s| crate::widgets::HistorySearchView {
                query: s.query.as_str(),
                matched: s.matched.is_some(),
            });
    let input = crate::widgets::InputWidget::new(&state.ui.input, styles)
        .focused(is_focused)
        .streaming(state.is_streaming())
        .prompt_suggestion(state.session.prompt_suggestions.last().map(String::as_str))
        .has_editable_queue(state.session.queued_commands.iter().any(|q| q.editable))
        .command_palette_filter(None)
        .history_search(history_search);

    frame.render_widget(input, area);
}

fn interaction_prompt_box_width(area_width: u16) -> u16 {
    area_width.min(constants::MAX_INTERACTION_PROMPT_WIDTH)
}

/// Center a fixed-width strip inside `area`, preserving full height. Unlike
/// `layout::centered_fixed_area` (which subtracts 2 from height for modal
/// margins), this keeps every row available for the prompt body.
fn center_horizontally(area: Rect, box_width: u16) -> Rect {
    let width = box_width.min(area.width);
    let x_offset = area.width.saturating_sub(width) / 2;
    Rect::new(area.x + x_offset, area.y, width, area.height)
}

fn interaction_prompt_height(state: &AppState, width: u16, max_height: u16) -> u16 {
    let Some(prompt) = state.ui.interaction.active_prompt.as_ref() else {
        return 0;
    };
    let styles = UiStyles::new(&state.ui.theme);
    if let PanePromptState::Question(q) = prompt {
        return question_prompt_max_height(q, width, styles)
            .min(max_height.saturating_sub(4))
            .max(3);
    }
    if let PanePromptState::Permission(p) = prompt
        && is_exit_plan_prompt(p)
    {
        let list_budget = p.choices.as_ref().map(Vec::len).unwrap_or(1);
        let lines = crate::presentation::request::exit_plan_prompt_lines(p, styles, list_budget);
        return wrapped_lines_height(&lines, width)
            .min(max_height.saturating_sub(4))
            .max(3);
    }
    let box_width = interaction_prompt_box_width(width);
    let Some(text_surface) = crate::surface_content::prompt_text_surface(prompt) else {
        return 0;
    };
    required_text_surface_height_for_box(text_surface, state, styles, box_width, max_height)
        .min(max_height.saturating_sub(4))
        .max(3)
}

fn question_prompt_max_height(
    q: &crate::state::QuestionPromptState,
    box_width: u16,
    styles: UiStyles<'_>,
) -> u16 {
    use crate::state::QuestionPage;

    // One clone reused across pages (this runs several times per frame while
    // a question prompt is open): `set_question_page` overwrites the page and
    // focus deterministically from the untouched `questions` data, so
    // re-pointing one projection copy is equivalent to cloning per page.
    let mut projected = q.clone();
    let mut max_height = 0;
    for idx in 0..q.questions.len() {
        projected.set_question_page(idx);
        let view = crate::presentation::request::project_question(&projected);
        max_height = max_height.max(view.desired_height(box_width, styles));
    }
    if q.questions.len() > 1 {
        projected.current_question = QuestionPage::Submit;
        projected.focus_target = crate::state::QuestionFocusTarget::SubmitAction(
            crate::state::SubmitAction::SubmitAnswers,
        );
        projected.sync_other_focus();
        let view = crate::presentation::request::project_question(&projected);
        max_height = max_height.max(view.desired_height(box_width, styles));
    }
    max_height
}

pub(crate) fn active_exit_plan_prompt(state: &AppState) -> Option<&PermissionPromptState> {
    match state.ui.interaction.active_prompt.as_ref() {
        Some(PanePromptState::Permission(p)) if is_exit_plan_prompt(p) => Some(p),
        _ => None,
    }
}

fn is_exit_plan_prompt(p: &PermissionPromptState) -> bool {
    matches!(p.detail, PermissionDetail::ExitPlanMode { .. })
}

fn wrapped_lines_height(lines: &[Line<'static>], width: u16) -> u16 {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let line_width: usize = line
                .spans
                .iter()
                .map(|span| text_width(span.content.as_ref()))
                .sum();
            line_width.saturating_add(width - 1) / width
        })
        .map(|rows| rows.max(1))
        .sum::<usize>() as u16
}

fn render_interaction_prompt(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    state: &AppState,
    styles: UiStyles<'_>,
) {
    if area.height == 0 {
        return;
    }
    let Some(prompt) = state.ui.interaction.active_prompt.as_ref() else {
        return;
    };
    // AskUserQuestion renders through the dedicated area-based widget, pinned to
    // the lower-left above the composer instead of horizontally centered like
    // the modal text prompts below.
    if let PanePromptState::Question(q) = prompt {
        let view = crate::presentation::request::project_question(q);
        frame.render_widget(Clear, area);
        frame.render_widget(
            coco_tui_ui::widgets::QuestionWidget::new(&view, styles),
            area,
        );
        return;
    }

    if let PanePromptState::Permission(p) = prompt
        && is_exit_plan_prompt(p)
    {
        let list_budget = p.choices.as_ref().map(Vec::len).unwrap_or(1);
        let lines = crate::presentation::request::exit_plan_prompt_lines(p, styles, list_budget);
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }

    let box_area = center_horizontally(area, interaction_prompt_box_width(area.width));
    if let PanePromptState::Permission(p) = prompt {
        let (title, lines, border_color) = crate::presentation::request::permission_styled_content(
            p,
            state.session.permission_mode,
            styles,
        );
        render_permission_box(
            frame,
            box_area,
            title,
            lines,
            border_color,
            state.ui.interaction.permission_scroll,
        );
        return;
    }

    let Some(text_surface) = crate::surface_content::prompt_text_surface(prompt) else {
        return;
    };
    let (title, body, border_color) =
        crate::surface_content::surface_content(text_surface, state, styles);
    let body = compact_prompt_body(&body, area.height.saturating_sub(2) as usize);
    frame.render_widget(Clear, box_area);
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        ),
        box_area,
    );
}

/// Render a permission prompt box whose command/detail body scrolls (offset
/// `scroll`, clamped here against the wrapped body height) while the trailing
/// action rows stay pinned to the bottom and always visible. Mirrors codex-rs'
/// approval overlay (scrollable content + fixed actions) instead of collapsing
/// the middle of a long command with an ellipsis.
fn render_permission_box(
    frame: &mut SurfaceFrame<'_>,
    box_area: Rect,
    title: String,
    lines: Vec<Line<'static>>,
    border_color: Color,
    scroll: u16,
) {
    let (body, actions) = split_body_and_actions(lines);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(box_area);
    let inner_w = inner.width;

    // Reserve rows for the actions at the bottom, always keeping >=1 row for
    // the body so a tall action list can't hide the command entirely.
    let action_h = (actions.len() as u16).min(inner.height.saturating_sub(1));
    let body_h = inner.height.saturating_sub(action_h);

    let body_total = wrapped_lines_height(&body, inner_w);
    let max_scroll = body_total.saturating_sub(body_h);
    let scroll = scroll.min(max_scroll);

    // Advertise the scroll affordance only when the body actually overflows.
    let title = if max_scroll > 0 {
        format!("{} — PgUp/PgDn ⇅", title.trim_end())
    } else {
        title
    };

    frame.render_widget(Clear, box_area);
    frame.render_widget(block.title(title), box_area);

    if body_h > 0 {
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            Rect::new(inner.x, inner.y, inner_w, body_h),
        );
    }
    if action_h > 0 {
        frame.render_widget(
            Paragraph::new(actions).wrap(Wrap { trim: false }),
            Rect::new(inner.x, inner.y + body_h, inner_w, action_h),
        );
    }
}

/// Split styled prompt lines into `(body, actions)` at the LAST blank
/// separator. The trailing block (decision rows + hint) is pinned; the leading
/// block (command / detail) scrolls. With no blank separator everything is body.
fn split_body_and_actions(lines: Vec<Line<'static>>) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let is_blank =
        |line: &Line<'static>| line.spans.iter().all(|span| span.content.trim().is_empty());
    match lines.iter().rposition(is_blank) {
        // Keep the blank separator as the body's trailing row so a prompt that
        // fits renders identically to the pre-scroll single-paragraph layout
        // (body flows from the top, actions sit directly below).
        Some(idx) if idx + 1 < lines.len() => (lines[..=idx].to_vec(), lines[idx + 1..].to_vec()),
        _ => (lines, Vec::new()),
    }
}

/// Shrink `items` to fit `max_lines`, shared by the styled (`Line`) and plain
/// (`&str`) prompt-compaction paths. Prefers keeping the whole trailing block
/// after the last blank separator (so a prompt's action rows always survive)
/// and otherwise keeps a head slice + an `ellipsis()` marker + a tail slice.
fn compact_sequence<T: Clone>(
    items: Vec<T>,
    max_lines: usize,
    is_blank: impl Fn(&T) -> bool,
    ellipsis: impl Fn() -> T,
) -> Vec<T> {
    if max_lines == 0 || items.len() <= max_lines {
        return items;
    }
    if max_lines == 1 {
        return items.into_iter().last().into_iter().collect();
    }
    if max_lines == 2 {
        let mut iter = items.into_iter();
        let first = iter.next();
        let last = iter.last().or_else(|| first.clone());
        return first.into_iter().chain(last).collect();
    }
    if let Some(blank_idx) = items.iter().rposition(&is_blank) {
        let tail_len = items.len().saturating_sub(blank_idx + 1);
        if tail_len > 0 {
            if tail_len < max_lines {
                let head_count = max_lines.saturating_sub(tail_len + 1);
                let mut compact = Vec::new();
                compact.extend(items.iter().take(head_count).cloned());
                compact.push(ellipsis());
                compact.extend(items.iter().skip(blank_idx + 1).cloned());
                return compact;
            }
            if tail_len == max_lines {
                return items.into_iter().skip(blank_idx + 1).collect();
            }
        }
    }
    let tail_count = (max_lines / 2).max(1);
    let head_count = max_lines.saturating_sub(tail_count + 1);
    let mut compact = Vec::new();
    compact.extend(items.iter().take(head_count).cloned());
    compact.push(ellipsis());
    compact.extend(
        items
            .iter()
            .skip(items.len().saturating_sub(tail_count))
            .cloned(),
    );
    compact
}

fn compact_prompt_body(body: &str, max_lines: usize) -> String {
    if max_lines == 0 || body.lines().count() <= max_lines {
        return body.to_string();
    }
    let lines = body.lines().map(str::to_owned).collect::<Vec<_>>();
    compact_sequence(
        lines,
        max_lines,
        |line| line.trim().is_empty(),
        || "...".to_string(),
    )
    .join("\n")
}

fn render_toasts(
    frame: &mut SurfaceFrame<'_>,
    area: Rect,
    toasts: &std::collections::VecDeque<crate::state::Toast>,
    styles: UiStyles<'_>,
) {
    let owned = toasts.iter().cloned().collect::<Vec<_>>();
    frame.render_widget(ToastWidget::new(&owned, styles), area);
}

#[cfg(test)]
#[path = "viewport.test.rs"]
mod tests;
