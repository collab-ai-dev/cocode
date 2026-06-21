//! Todo/Plan panel projection — V2 plan_tasks or V1 todos_by_agent.
//!
//! ## V1/V2 mutual exclusion
//!
//! V2 wins when `plan_tasks` is non-empty; otherwise V1 wins when
//! `todos_by_agent` is non-empty. Both empty → no rows.
//!
//! ## Priority sort
//!
//! Within each family, rows are ordered:
//!   1. **in_progress** (highest — the active focus)
//!   2. **pending**
//!   3. **completed** (lowest — context, not action)
//!
//! "Recently completed (< 30 s)" rows promote above pending. That
//! promotion needs a completion-timestamp side-cache in `SessionState`;
//! deferred until the cache lands (see P3 follow-up). Without it,
//! completed rows always trail pending — closest stable behaviour.
//!
//! ## Glyphs
//!
//! - `✔` (completed, success-tone)
//! - `◼` (in_progress, accent-tone)
//! - `☐` (pending, dim)

use crate::i18n::t;
use crate::presentation::activity::ActivityLine;
use crate::presentation::activity::ActivitySpan;
use crate::presentation::activity::ActivityTone;
use crate::state::AppState;
use coco_types::TaskListStatus;

const ICON_COMPLETED: &str = "✔";
const ICON_IN_PROGRESS: &str = "◼";
const ICON_PENDING: &str = "☐";

/// Promote tasks completed within this window above pending.
const RECENT_COMPLETED_TTL_MS: i64 = 30_000;

/// Hide the entire panel this long after every plan task became `Completed`.
const HIDE_DELAY_MS: i64 = 5_000;

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

    out.push(ActivityLine::section(
        t!("plan_panel.section_tasks").to_string(),
    ));

    // Sort indices by priority. Recently-completed tasks (< 30s) lift
    // above pending so the user sees the most recent wins at the top.
    let mut indices: Vec<usize> = (0..state.session.plan_tasks.len()).collect();
    indices.sort_by_key(|&i| {
        let task = &state.session.plan_tasks[i];
        rank_v2(task, &state.ui.ephemeral.task_completion_timestamps, now)
    });

    for i in indices {
        let task = &state.session.plan_tasks[i];
        let (icon, tone) = icon_for_v2(task.status);
        let owner = task
            .owner
            .as_deref()
            .map(|o| format!(" ({o})"))
            .unwrap_or_default();
        let blocked = if task.blocked_by.is_empty() {
            String::new()
        } else {
            format!(" [blocked by {}]", task.blocked_by.join(", "))
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
    out.push(ActivityLine::blank());
}

fn append_v1(state: &AppState, out: &mut Vec<ActivityLine>) {
    out.push(ActivityLine::section(
        t!("plan_panel.section_todos").to_string(),
    ));

    let mut keys: Vec<&String> = state.session.todos_by_agent.keys().collect();
    keys.sort();
    for key in keys {
        let items = &state.session.todos_by_agent[key];
        if items.is_empty() {
            continue;
        }
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
    out.push(ActivityLine::blank());
}

/// Composite rank: recently-completed (<30s) → in_progress → pending →
/// older-completed.
fn rank_v2(
    task: &coco_types::TaskRecord,
    completion_ts: &std::collections::HashMap<String, i64>,
    now: i64,
) -> u8 {
    match task.status {
        TaskListStatus::Completed => match completion_ts.get(task.id.as_str()) {
            Some(&ts) if now.saturating_sub(ts) < RECENT_COMPLETED_TTL_MS => 0,
            _ => 3,
        },
        TaskListStatus::InProgress => 1,
        TaskListStatus::Pending => 2,
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
