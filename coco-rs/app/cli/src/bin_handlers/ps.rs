//! `coco ps` — enumerate live background sessions, merged with durable
//! terminal job records.
//!
//! The PID registry ([`coco_session`]) deletes a session's `<pid>.json`
//! on exit, so it carries no terminal state. The `coco-tasks`
//! [`JobStore`](coco_tasks::JobStore) persists a terminal record keyed
//! by `session_id`. This handler is the merge site — it lives in the CLI
//! layer because that is the only layer that depends on *both*
//! `coco-session` and `coco-tasks` (the layer graph forbids
//! `coco-session → coco-tasks`).

use std::collections::HashMap;
use std::path::Path;

use coco_session::PsEntry;
use coco_session::PsViewState;
use coco_session::SessionStatus;
use coco_session::TerminalJobOutcome;
use coco_session::collect_ps_entries;
use coco_tasks::JobState;
use coco_tasks::JobStore;
use coco_types::TaskStatus;

/// Map a terminal [`TaskStatus`] to the `coco ps` lifecycle outcome.
/// Returns `None` for non-terminal statuses (`Pending` / `Running`).
fn terminal_outcome(status: TaskStatus) -> Option<TerminalJobOutcome> {
    match status {
        TaskStatus::Completed => Some(TerminalJobOutcome::Done),
        TaskStatus::Failed => Some(TerminalJobOutcome::Failed),
        TaskStatus::Killed => Some(TerminalJobOutcome::Stopped),
        TaskStatus::Pending | TaskStatus::Running => None,
    }
}

/// True when the live transport status outranks any job outcome — a
/// busy worker is `Working` and a waiting one is `Blocked` regardless of
/// a stale terminal record (mirrors [`coco_session::view_state`]'s
/// precedence rungs 1-2).
fn live_status_wins(entry: &PsEntry) -> bool {
    entry.status == Some(SessionStatus::Busy)
        || entry.status == Some(SessionStatus::Waiting)
        || entry.waiting_for.is_some()
}

/// Collect the live PID sweep and merge durable [`JobStore`] terminal
/// records by `session_id`. With `include_all`, process-less terminal
/// job records that have no live session row are appended too.
pub fn collect_with_jobs(config_home: &Path, include_all: bool) -> Vec<PsEntry> {
    let mut entries = collect_ps_entries(config_home, include_all);

    let jobs: HashMap<String, JobState> = JobStore::new(config_home)
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|j| (j.session_id.clone(), j))
        .collect();
    if jobs.is_empty() {
        return entries;
    }

    // Override live rows that have a settled terminal job and aren't
    // currently busy/waiting.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in &mut entries {
        seen.insert(entry.session_id.clone());
        if live_status_wins(entry) {
            continue;
        }
        if let Some(outcome) = jobs
            .get(&entry.session_id)
            .and_then(|j| terminal_outcome(j.status))
        {
            entry.state = match outcome {
                TerminalJobOutcome::Done => PsViewState::Done,
                TerminalJobOutcome::Failed => PsViewState::Failed,
                TerminalJobOutcome::Stopped => PsViewState::Stopped,
            };
        }
    }

    // With --all, surface process-less terminal jobs that have no live row.
    if include_all {
        let mut extra: Vec<PsEntry> = jobs
            .into_values()
            .filter(|j| !seen.contains(&j.session_id))
            .filter_map(|j| terminal_outcome(j.status).map(|o| (j, o)))
            .map(|(j, outcome)| PsEntry {
                pid: 0,
                id: j.session_id.clone(),
                cwd: j.cwd,
                kind: j.kind,
                started_at: j.created_at,
                session_id: j.session_id,
                name: j.name,
                status: None,
                waiting_for: None,
                state: match outcome {
                    TerminalJobOutcome::Done => PsViewState::Done,
                    TerminalJobOutcome::Failed => PsViewState::Failed,
                    TerminalJobOutcome::Stopped => PsViewState::Stopped,
                },
            })
            .collect();
        entries.append(&mut extra);
        entries.sort_by_key(|e| e.started_at);
    }

    entries
}

#[cfg(test)]
#[path = "ps.test.rs"]
mod tests;
