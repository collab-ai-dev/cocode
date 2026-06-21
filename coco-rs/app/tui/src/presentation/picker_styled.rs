//! Styled renderers for the list dialogs migrated onto the reusable
//! `coco_tui_ui::widgets::render_select_list` widget. Each returns
//! `(title, body lines, border)` for the styled-modal path in
//! `surface/modal.rs`, replacing the monochrome `(String, String, Color)`
//! builders in `picker.rs`. The list rows get a colored `❯` cursor; chrome
//! (intro / filter / hint lines) is dim.

use ratatui::prelude::*;

use crate::i18n::t;
use crate::presentation::layout::truncate_to_width;
use crate::presentation::picker::first_line_preview;
use crate::presentation::picker::memory_row_kind_tag;
use crate::presentation::picker::memory_scope_tag;
use crate::state::AppState;
use crate::state::BackgroundTasksState;
use crate::state::CopyPickerSelection;
use crate::state::CopyPickerState;
use crate::state::ExportState;
use crate::state::GlobalSearchState;
use crate::state::MemoryDialogState;
use crate::state::QuickOpenState;
use crate::state::SessionBrowserState;
use crate::state::TeamRosterState;
use crate::state::session::SubagentInstance;
use crate::state::session::TaskEntry;
use crate::state::session::TaskEntryKind;
use crate::state::session::TaskEntryStatus;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::render_select_list;

/// A dim chrome line (intro / filter / hint).
fn dim_line(text: impl Into<String>, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::default().fg(styles.dim())))
}

/// A body-text line (default foreground).
fn text_line(text: impl Into<String>, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(styles.text()),
    ))
}

/// `SelectListStyle` for these dialogs: unnumbered, scrolls past `visible`.
fn list_style(visible: usize) -> SelectListStyle {
    SelectListStyle {
        numbered: false,
        visible_count: visible.max(1),
    }
}

/// `/export` — pick an export format. Flat single-select list.
pub(crate) fn export_lines(
    e: &ExportState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let items: Vec<SelectItem> = e
        .formats
        .iter()
        .map(|fmt| SelectItem::new(fmt.label()))
        .collect();
    let mut lines = vec![
        dim_line(t!("dialog.select_format"), styles),
        Line::default(),
    ];
    lines.extend(render_select_list(
        &items,
        e.selected.max(0) as usize,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_nav_export_cancel"), styles));
    (
        t!("dialog.title_export").to_string(),
        lines,
        styles.primary(),
    )
}

/// Teams roster — the leader cycles a focused teammate's permission mode.
/// Rows list the running teammates; the pill below shows the mode about to
/// be applied on Enter.
pub(crate) fn team_roster_lines(
    r: &TeamRosterState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.title_team_roster").to_string();
    if r.members.is_empty() {
        return (
            title,
            vec![dim_line(t!("dialog.team_roster_empty"), styles)],
            styles.primary(),
        );
    }
    let items: Vec<SelectItem> = r
        .members
        .iter()
        .map(|m| {
            // Each row shows the teammate's OWN current mode (TS renders
            // `teammate.mode` per row) so divergent modes are visible at a
            // glance.
            let mode = crate::update::permission_mode_label(m.mode);
            let label = if m.agent_type.is_empty() {
                format!("{} · {mode}", m.name)
            } else {
                format!("{} ({}) · {mode}", m.name, m.agent_type)
            };
            SelectItem::new(label)
        })
        .collect();
    let mut lines = vec![
        dim_line(
            t!("dialog.team_roster_team", team = r.team_name.as_str()),
            styles,
        ),
        Line::default(),
    ];
    lines.extend(render_select_list(
        &items,
        r.selected.min(items.len().saturating_sub(1)),
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    // The pill reflects the FOCUSED member's mode (the one Left/Right edits).
    let focused_mode = r
        .members
        .get(r.selected)
        .map(|m| m.mode)
        .unwrap_or(coco_types::PermissionMode::Default);
    lines.push(Line::from(vec![
        Span::styled(
            t!("dialog.team_roster_set_mode").to_string(),
            Style::default().fg(styles.dim()),
        ),
        Span::styled(
            crate::update::permission_mode_label(focused_mode),
            Style::default().fg(styles.accent()),
        ),
    ]));
    lines.push(dim_line(t!("dialog.hints_team_roster"), styles));
    (title, lines, styles.primary())
}

/// `/memory` — pick a memory file to edit. Flat single-select list.
pub(crate) fn memory_dialog_lines(
    m: &MemoryDialogState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.title_memory").to_string();
    if m.entries.is_empty() {
        return (
            title,
            vec![dim_line(t!("dialog.memory_no_files"), styles)],
            styles.primary(),
        );
    }
    let items: Vec<SelectItem> = m
        .entries
        .iter()
        .map(|e| {
            SelectItem::new(format!(
                "{} {} {}",
                memory_row_kind_tag(e.row_kind),
                memory_scope_tag(e.scope),
                e.label
            ))
        })
        .collect();
    let mut lines = vec![
        dim_line(t!("dialog.memory_select"), styles),
        Line::default(),
    ];
    lines.extend(render_select_list(
        &items,
        m.selected.max(0) as usize,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_nav_select_cancel"), styles));
    (title, lines, styles.primary())
}

/// `/quick-open` — fuzzy file picker. Filtered flat list, capped at 15 rows.
pub(crate) fn quick_open_lines(
    q: &QuickOpenState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let items: Vec<SelectItem> = q.files.iter().map(SelectItem::new).collect();
    let filter = if q.filter.is_empty() {
        dim_line(t!("dialog.type_file_name"), styles)
    } else {
        dim_line(t!("dialog.open_prefix", text = q.filter.as_str()), styles)
    };
    let mut lines = vec![filter, Line::default()];
    lines.extend(render_select_list(
        &items,
        q.selected.max(0) as usize,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_enter_open_cancel"), styles));
    (
        t!("dialog.title_quick_open").to_string(),
        lines,
        styles.primary(),
    )
}

/// `/resume` — session browser. Filtered flat list.
pub(crate) fn session_browser_lines(
    s: &SessionBrowserState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.title_sessions").to_string();
    let filter_lower = s.filter.to_lowercase();
    let items: Vec<SelectItem> = s
        .sessions
        .iter()
        .filter(|sess| filter_lower.is_empty() || sess.label.to_lowercase().contains(&filter_lower))
        .map(|session| {
            SelectItem::new(format!(
                "{} — {}{} — {}",
                session.label,
                session.message_count,
                t!("dialog.sessions_item_suffix"),
                session.created_at
            ))
        })
        .collect();
    if items.is_empty() {
        return (
            title,
            vec![dim_line(t!("dialog.no_saved_sessions"), styles)],
            styles.primary(),
        );
    }
    let filter = if s.filter.is_empty() {
        dim_line(t!("dialog.type_filter_sessions"), styles)
    } else {
        dim_line(t!("dialog.filter_prefix", text = s.filter.as_str()), styles)
    };
    let mut lines = vec![filter, Line::default()];
    lines.extend(render_select_list(
        &items,
        s.selected.max(0) as usize,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_nav_resume_cancel"), styles));
    (title, lines, styles.primary())
}

/// `/search` — global search. Query line + status + flat result list (cap 20).
pub(crate) fn global_search_lines(
    g: &GlobalSearchState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let query_text = if g.query.is_empty() {
        t!("dialog.type_search").to_string()
    } else {
        t!("dialog.search_prefix", text = g.query.as_str()).to_string()
    };
    let mut lines = vec![dim_line(query_text, styles)];
    if g.is_searching {
        lines.push(dim_line(t!("dialog.searching"), styles));
    } else if g.results.is_empty() && !g.query.is_empty() {
        lines.push(dim_line(t!("dialog.no_results"), styles));
    }
    lines.push(Line::default());

    let items: Vec<SelectItem> = g
        .results
        .iter()
        .map(|r| SelectItem::new(format!("{}:{} {}", r.file, r.line_number, r.content.trim())))
        .collect();
    lines.extend(render_select_list(
        &items,
        g.selected.max(0) as usize,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.esc_cancel"), styles));
    (
        t!("dialog.title_global_search").to_string(),
        lines,
        styles.primary(),
    )
}

/// `/copy` — pick what to copy (full response, a code block, or "always").
/// The selection is an enum; map it to the flat row index.
pub(crate) fn copy_picker_lines(
    cp: &CopyPickerState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let full_lines = cp.full_text.matches('\n').count() + 1;
    let chars = cp.full_text.chars().count();
    let mut items = vec![SelectItem::new(
        t!(
            "copy.picker_full_response",
            chars = chars,
            lines = full_lines
        )
        .to_string(),
    )];
    for block in &cp.code_blocks {
        let lang = block.lang.as_deref().unwrap_or("text");
        let block_chars = block.code.chars().count();
        let preview = first_line_preview(&block.code, 60);
        items.push(SelectItem::new(
            t!(
                "copy.picker_code_block",
                lang = lang,
                chars = block_chars,
                preview = preview.as_str(),
            )
            .to_string(),
        ));
    }
    items.push(SelectItem::new(t!("copy.picker_always").to_string()));

    let selected = match cp.selected {
        CopyPickerSelection::Full => 0,
        CopyPickerSelection::CodeBlock(i) => 1 + i,
        CopyPickerSelection::Always => 1 + cp.code_blocks.len(),
    };

    let mut lines = vec![
        dim_line(t!("dialog.copy_picker_prompt"), styles),
        Line::default(),
    ];
    lines.extend(render_select_list(
        &items,
        selected,
        &list_style(list_budget),
        styles,
    ));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.hints_copy_picker"), styles));
    (
        t!("dialog.title_copy_picker", age = cp.message_age + 1).to_string(),
        lines,
        styles.primary(),
    )
}

/// Background-tasks dialog — running shells/agents grouped into dim-headed
/// sections, or a single task's detail view. Selection is a flat index into
/// `running_background_tasks()` (shared with `update::background_tasks`), and
/// rows render in that same grouped order so the `❯` cursor lands on the row
/// the intercept acts on. Agent rows join the richer `SubagentInstance` by
/// `agent_id == task_id` for the colored type badge and live tool activity.
pub(crate) fn background_tasks_lines(
    bt: &BackgroundTasksState,
    state: &AppState,
    styles: UiStyles<'_>,
    _list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let now_ms = state.clock.now_ms();
    let rows = state.session.running_background_tasks();
    if let Some(task_id) = bt.detail.as_deref() {
        background_tasks_detail_lines(&rows, task_id, state, now_ms, styles)
    } else {
        background_tasks_list_lines(&rows, bt.selected, state, now_ms, styles)
    }
}

fn background_tasks_list_lines(
    rows: &[&TaskEntry],
    selected: usize,
    state: &AppState,
    now_ms: i64,
    styles: UiStyles<'_>,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.background_tasks_title").to_string();
    if rows.is_empty() {
        return (
            title,
            vec![dim_line(t!("dialog.background_empty"), styles)],
            styles.primary(),
        );
    }
    let selected = selected.min(rows.len() - 1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(pill) = crate::status_bar::background_pill_label(state) {
        lines.push(dim_line(pill, styles));
        lines.push(Line::default());
    }
    let mut current_kind: Option<TaskEntryKind> = None;
    for (i, task) in rows.iter().enumerate() {
        if current_kind != Some(task.kind) {
            current_kind = Some(task.kind);
            lines.push(dim_line(section_header(task.kind), styles));
        }
        lines.push(background_task_row(
            task,
            i == selected,
            state,
            now_ms,
            styles,
        ));
    }
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.background_list_hints"), styles));
    (title, lines, styles.primary())
}

/// One task row: `❯ <type> <description>   <runtime> · <activity>` for agents,
/// `❯ $ <command>   <runtime>` for shells.
fn background_task_row(
    task: &TaskEntry,
    selected: bool,
    state: &AppState,
    now_ms: i64,
    styles: UiStyles<'_>,
) -> Line<'static> {
    let label_color = if selected {
        styles.accent()
    } else {
        styles.text()
    };
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        if selected { "❯ " } else { "  " }.to_string(),
        Style::default().fg(styles.accent()),
    )];

    let subagent = find_subagent(state, &task.task_id);
    match task.kind {
        TaskEntryKind::Agent => {
            if let Some(sub) = subagent
                && !sub.agent_type.is_empty()
            {
                let color = sub
                    .color
                    .map(crate::widgets::suggestion_popup::agent_color_to_ratatui)
                    .unwrap_or_else(|| styles.accent());
                spans.push(Span::styled(
                    format!("{} ", sub.agent_type),
                    Style::default().fg(color),
                ));
            }
            spans.push(Span::styled(
                truncate_to_width(&task.description, 48),
                Style::default().fg(label_color),
            ));
        }
        TaskEntryKind::Shell | TaskEntryKind::Other => {
            spans.push(Span::styled(
                "$ ".to_string(),
                Style::default().fg(styles.dim()),
            ));
            spans.push(Span::styled(
                truncate_to_width(&task.description, 52),
                Style::default().fg(label_color),
            ));
        }
    }

    spans.push(Span::styled(
        format!("  {}", format_runtime(now_ms - task.started_at_ms)),
        Style::default().fg(styles.dim()),
    ));

    // Latest tool activity for agents (live feed; shells have none plumbed).
    if let Some(act) = subagent.and_then(|s| s.recent_activities.last()) {
        let activity = act.summary.clone().unwrap_or_else(|| act.tool_name.clone());
        spans.push(Span::styled(
            format!(" · {}", truncate_to_width(&activity, 28)),
            Style::default().fg(styles.dim()),
        ));
    }
    Line::from(spans)
}

fn background_tasks_detail_lines(
    rows: &[&TaskEntry],
    task_id: &str,
    state: &AppState,
    now_ms: i64,
    styles: UiStyles<'_>,
) -> (String, Vec<Line<'static>>, Color) {
    let task = rows.iter().find(|t| t.task_id == task_id).copied();
    let (title, status, runtime, command, kind) = match task {
        Some(t) => (
            t!(detail_title_key(t.kind)).to_string(),
            t!(task_status_key(t.status)).to_string(),
            format_runtime(now_ms - t.started_at_ms),
            t.description.clone(),
            Some(t.kind),
        ),
        None => (
            t!("dialog.task_details_title").to_string(),
            t!("dialog.background_status_ended").to_string(),
            "—".to_string(),
            "—".to_string(),
            None,
        ),
    };

    let mut lines = vec![
        text_line(t!("dialog.background_status", status = status), styles),
        text_line(t!("dialog.background_runtime", runtime = runtime), styles),
        text_line(t!("dialog.background_command", command = command), styles),
        Line::default(),
    ];

    if matches!(kind, Some(TaskEntryKind::Agent)) {
        lines.push(dim_line(t!("dialog.background_activity_label"), styles));
        let activities = find_subagent(state, task_id)
            .map(|s| s.recent_activities.as_slice())
            .unwrap_or(&[]);
        if activities.is_empty() {
            lines.push(dim_line(t!("dialog.background_no_activity"), styles));
        } else {
            for act in activities {
                let mut spans = vec![
                    Span::styled(" · ".to_string(), Style::default().fg(styles.dim())),
                    Span::styled(act.tool_name.clone(), Style::default().fg(styles.text())),
                ];
                if let Some(summary) = &act.summary {
                    spans.push(Span::styled(
                        format!("  {}", truncate_to_width(summary, 40)),
                        Style::default().fg(styles.dim()),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
    } else {
        lines.push(dim_line(t!("dialog.background_output_label"), styles));
        lines.push(dim_line(t!("dialog.background_output_empty"), styles));
    }

    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.background_detail_hints"), styles));
    (title, lines, styles.primary())
}

fn find_subagent<'a>(state: &'a AppState, task_id: &str) -> Option<&'a SubagentInstance> {
    state
        .session
        .subagents
        .iter()
        .find(|a| a.agent_id == task_id)
}

fn section_header(kind: TaskEntryKind) -> String {
    match kind {
        TaskEntryKind::Agent => t!("dialog.background_section_agents").to_string(),
        TaskEntryKind::Shell => t!("dialog.background_section_shells").to_string(),
        TaskEntryKind::Other => t!("dialog.background_section_other").to_string(),
    }
}

fn detail_title_key(kind: TaskEntryKind) -> &'static str {
    match kind {
        TaskEntryKind::Shell => "dialog.shell_details_title",
        TaskEntryKind::Agent => "dialog.agent_details_title",
        TaskEntryKind::Other => "dialog.task_details_title",
    }
}

fn task_status_key(status: TaskEntryStatus) -> &'static str {
    match status {
        TaskEntryStatus::Running => "task_status.running",
        TaskEntryStatus::Completed => "task_status.completed",
        TaskEntryStatus::Failed => "task_status.failed",
        TaskEntryStatus::Stopped => "task_status.stopped",
    }
}

/// "7h 19m 32s" / "19m 32s" / "32s" from an elapsed-millis count.
fn format_runtime(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
#[path = "picker_styled.test.rs"]
mod tests;
