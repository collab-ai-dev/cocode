//! Todo/Plan panel projection — V2 plan_tasks or V1 todos_by_agent.
//!
//! ## V1/V2 mutual exclusion
//!
//! INTENTIONAL PRODUCT DIVERGENCE: Claude Code TS keeps V1 `TodoWrite` out of
//! the expanded V2 task panel and surfaces it through summary/status state.
//! coco-rs uses one right-rail task panel for both V2 `plan_tasks` and V1
//! `todos_by_agent` so multi-provider sessions have a single visible planning
//! surface. Row rendering stays aligned with TS by omitting inner list titles;
//! the outer right-rail panel title provides the grouping.
//!
//! V2 wins when `plan_tasks` is non-empty; otherwise V1 wins when
//! `todos_by_agent` is non-empty. Both empty → no rows.
//!
//! ## V2 ordering
//!
//! Mirrors Claude Code TS `TaskListV2`: when the list fits, rows are ordered by
//! task id. When the list is truncated, visible rows are selected in this
//! priority order: recently completed, in_progress, pending, older completed.
//! Pending rows with unresolved blockers trail unblocked pending rows, and the
//! hidden summary counts the truncated tail by status.
//!
//! "Recently completed (< 30 s)" rows promote above pending. Completion
//! timestamps live in `UiEphemeralState` and are stamped by the
//! `TaskPanelChanged` handler when a task first transitions to completed.
//!
//! ## Glyphs
//!
//! - `✔` (completed, success-tone)
//! - `◼` (in_progress, accent-tone)
//! - `☐` (pending, dim)

use crate::presentation::activity::ActivityLine;
use crate::presentation::activity::ActivitySpan;
use crate::presentation::activity::ActivityTone;
use crate::state::AppState;
use coco_types::TaskListStatus;
use std::collections::HashSet;

const ICON_COMPLETED: &str = "✔";
const ICON_IN_PROGRESS: &str = "◼";
const ICON_PENDING: &str = "☐";

/// Promote tasks completed within this window above pending.
const RECENT_COMPLETED_TTL_MS: i64 = 30_000;

/// Hide the entire panel this long after every plan task became `Completed`.
const HIDE_DELAY_MS: i64 = 5_000;
/// TS `TaskListV2` caps the expanded inline task list at five visible
/// task rows under truncation. coco-rs does not currently route terminal
/// row count into this projection, so we use the TS maximum as the
/// stable cap and keep the same priority/hidden-count semantics.
const MAX_VISIBLE_V2_TASKS: usize = 5;

/// Render the todo/plan panel section into `out` if state has content.
///
/// `out` is appended to in-place so callers can compose the panel with
/// preceding/trailing sections (running tasks, etc.).
///
/// DIVERGE: coco-rs auto-detects V1 vs V2: V2 wins when its content is
/// populated, else V1. The auto-detect is deliberate because (a) coco-rs
/// has no `COCO_TASKS_V2_ENABLED` env + settings field and (b) the engine
/// only emits one shape at a time, so "whichever has content" is the only
/// state that matters in practice. Add a settings flag here when users
/// need to suppress V2 even when populated.
pub(crate) fn append_lines(state: &AppState, out: &mut Vec<ActivityLine>) {
    if !state.session.plan_tasks.is_empty() {
        append_v2(state, out);
    } else if !state.session.todos_by_agent.is_empty() {
        append_v1(state, out);
    }
}

fn append_v2(state: &AppState, out: &mut Vec<ActivityLine>) {
    let now = state.clock.now_ms();

    // 5s auto-hide: once every plan task is completed and the anchor
    // has aged past HIDE_DELAY_MS, the panel suppresses itself entirely
    // so the user gets a brief celebration and then a clean composer.
    if let Some(since) = state.ui.ephemeral.tasks_all_completed_since_ms
        && now.saturating_sub(since) >= HIDE_DELAY_MS
    {
        return;
    }

    let unresolved_ids: HashSet<&str> = state
        .session
        .plan_tasks
        .iter()
        .filter(|task| task.status != TaskListStatus::Completed)
        .map(|task| task.id.as_str())
        .collect();

    let (indices, hidden_summary) = visible_v2_indices(state, &unresolved_ids, now);

    for i in indices {
        let task = &state.session.plan_tasks[i];
        let (icon, tone) = icon_for_v2(task.status);
        let owner = task
            .owner
            .as_deref()
            .map(|o| format!(" ({o})"))
            .unwrap_or_default();
        let open_blockers: Vec<&str> = task
            .blocked_by
            .iter()
            .filter_map(|id| unresolved_ids.contains(id.as_str()).then_some(id.as_str()))
            .collect();
        let blocked = if open_blockers.is_empty() {
            String::new()
        } else {
            format!(" [blocked by {}]", open_blockers.join(", "))
        };
        out.push(ActivityLine {
            spans: vec![
                ActivitySpan::raw("  "),
                ActivitySpan::tone(format!("{icon} "), tone),
                ActivitySpan::tone(format!("#{} ", task.id), ActivityTone::Dim),
                ActivitySpan::raw(task.subject.clone()),
                ActivitySpan::tone(owner, ActivityTone::Dim),
                ActivitySpan::tone(blocked, ActivityTone::Warning),
            ],
        });
    }
    if let Some(summary) = hidden_summary {
        out.push(ActivityLine::text(
            format!("  {summary}"),
            ActivityTone::Dim,
        ));
    }
    out.push(ActivityLine::blank());
}

fn append_v1(state: &AppState, out: &mut Vec<ActivityLine>) {
    let mut emitted = false;
    let mut keys: Vec<&String> = state.session.todos_by_agent.keys().collect();
    keys.sort();
    for key in keys {
        let items = &state.session.todos_by_agent[key];
        if items.is_empty() {
            continue;
        }
        emitted = true;
        out.push(ActivityLine::text(format!("  [{key}]"), ActivityTone::Dim));

        // Sort by status priority. V1 status is a free-form string, so
        // we map it onto the same rank as V2 for consistency.
        let mut indices: Vec<usize> = (0..items.len()).collect();
        indices.sort_by_key(|&i| status_rank_v1(items[i].status.as_str()));

        for i in indices {
            let item = &items[i];
            let (icon, tone) = icon_for_v1(item.status.as_str());
            out.push(ActivityLine {
                spans: vec![
                    ActivitySpan::raw("    "),
                    ActivitySpan::tone(format!("{icon} "), tone),
                    ActivitySpan::raw(item.content.clone()),
                ],
            });
        }
    }
    if emitted {
        out.push(ActivityLine::blank());
    }
}

fn visible_v2_indices(
    state: &AppState,
    unresolved_ids: &HashSet<&str>,
    now: i64,
) -> (Vec<usize>, Option<String>) {
    let tasks = &state.session.plan_tasks;
    if tasks.len() <= MAX_VISIBLE_V2_TASKS {
        let mut indices: Vec<usize> = (0..tasks.len()).collect();
        indices.sort_by(|&a, &b| compare_task_ids(&tasks[a].id, &tasks[b].id));
        return (indices, None);
    }

    let mut recent_completed = Vec::new();
    let mut older_completed = Vec::new();
    let mut in_progress = Vec::new();
    let mut pending = Vec::new();

    for (i, task) in tasks.iter().enumerate() {
        match task.status {
            TaskListStatus::Completed => {
                let recent = state
                    .ui
                    .ephemeral
                    .task_completion_timestamps
                    .get(task.id.as_str())
                    .is_some_and(|ts| now.saturating_sub(*ts) < RECENT_COMPLETED_TTL_MS);
                if recent {
                    recent_completed.push(i);
                } else {
                    older_completed.push(i);
                }
            }
            TaskListStatus::InProgress => in_progress.push(i),
            TaskListStatus::Pending => pending.push(i),
        }
    }

    for group in [
        &mut recent_completed,
        &mut older_completed,
        &mut in_progress,
    ] {
        group.sort_by(|&a, &b| compare_task_ids(&tasks[a].id, &tasks[b].id));
    }
    pending.sort_by(|&a, &b| {
        let a_blocked = tasks[a]
            .blocked_by
            .iter()
            .any(|id| unresolved_ids.contains(id.as_str()));
        let b_blocked = tasks[b]
            .blocked_by
            .iter()
            .any(|id| unresolved_ids.contains(id.as_str()));
        a_blocked
            .cmp(&b_blocked)
            .then_with(|| compare_task_ids(&tasks[a].id, &tasks[b].id))
    });

    let ordered: Vec<usize> = recent_completed
        .into_iter()
        .chain(in_progress)
        .chain(pending)
        .chain(older_completed)
        .collect();
    let visible = ordered
        .iter()
        .copied()
        .take(MAX_VISIBLE_V2_TASKS)
        .collect::<Vec<_>>();
    let hidden = &ordered[visible.len()..];
    (visible, hidden_v2_summary(tasks, hidden))
}

fn hidden_v2_summary(tasks: &[coco_types::TaskRecord], hidden: &[usize]) -> Option<String> {
    if hidden.is_empty() {
        return None;
    }
    let pending = hidden
        .iter()
        .filter(|&&i| tasks[i].status == TaskListStatus::Pending)
        .count();
    let in_progress = hidden
        .iter()
        .filter(|&&i| tasks[i].status == TaskListStatus::InProgress)
        .count();
    let completed = hidden
        .iter()
        .filter(|&&i| tasks[i].status == TaskListStatus::Completed)
        .count();
    let mut parts = Vec::new();
    if in_progress > 0 {
        parts.push(format!("{in_progress} in progress"));
    }
    if pending > 0 {
        parts.push(format!("{pending} pending"));
    }
    if completed > 0 {
        parts.push(format!("{completed} completed"));
    }
    Some(format!("… +{}", parts.join(", ")))
}

fn compare_task_ids(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

fn status_rank_v1(s: &str) -> u8 {
    match s {
        "in_progress" => 0,
        "pending" => 1,
        "completed" => 2,
        _ => 3,
    }
}

fn icon_for_v2(s: TaskListStatus) -> (&'static str, ActivityTone) {
    match s {
        TaskListStatus::Completed => (ICON_COMPLETED, ActivityTone::Completed),
        TaskListStatus::InProgress => (ICON_IN_PROGRESS, ActivityTone::Accent),
        TaskListStatus::Pending => (ICON_PENDING, ActivityTone::Dim),
    }
}

fn icon_for_v1(s: &str) -> (&'static str, ActivityTone) {
    match s {
        "completed" => (ICON_COMPLETED, ActivityTone::Completed),
        "in_progress" => (ICON_IN_PROGRESS, ActivityTone::Accent),
        "pending" => (ICON_PENDING, ActivityTone::Dim),
        _ => ("?", ActivityTone::Dim),
    }
}

#[cfg(test)]
#[path = "todo_panel.test.rs"]
mod tests;
