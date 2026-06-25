//! Tests for the `coco ps` JobStore merge.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use coco_session::SessionKind;
use coco_session::SessionRegistry;
use coco_tasks::job_store::now_ms;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn job(session_id: &str, status: TaskStatus) -> JobState {
    let now = now_ms();
    JobState {
        id: format!("job-{session_id}"),
        session_id: session_id.to_string(),
        cwd: std::path::PathBuf::from("/work"),
        kind: SessionKind::DaemonWorker,
        created_at: now,
        updated_at: now,
        status,
        name: None,
        intent: None,
        error: None,
    }
}

#[test]
fn terminal_outcome_maps_terminal_statuses() {
    assert_eq!(
        terminal_outcome(TaskStatus::Completed),
        Some(TerminalJobOutcome::Done)
    );
    assert_eq!(
        terminal_outcome(TaskStatus::Failed),
        Some(TerminalJobOutcome::Failed)
    );
    assert_eq!(
        terminal_outcome(TaskStatus::Killed),
        Some(TerminalJobOutcome::Stopped)
    );
    assert_eq!(terminal_outcome(TaskStatus::Pending), None);
    assert_eq!(terminal_outcome(TaskStatus::Running), None);
}

#[test]
fn live_row_with_terminal_job_is_overridden() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session_id = "sid-merge";

    // Live self-pid row, status None → would map to Working.
    let _registry = SessionRegistry::register(cfg.path(), session_id, cwd.path(), None)
        .unwrap()
        .unwrap();
    // Durable terminal record for the same session.
    JobStore::new(cfg.path())
        .write(&job(session_id, TaskStatus::Failed))
        .unwrap();

    let entries = collect_with_jobs(cfg.path(), /*include_all*/ false);
    let row = entries
        .iter()
        .find(|e| e.session_id == session_id)
        .expect("self row present");
    assert_eq!(row.state, PsViewState::Failed);
}

#[test]
fn busy_live_row_outranks_terminal_job() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session_id = "sid-busy";

    let registry = SessionRegistry::register(cfg.path(), session_id, cwd.path(), None)
        .unwrap()
        .unwrap();
    registry.update_session_activity(Some(SessionStatus::Busy), None);
    JobStore::new(cfg.path())
        .write(&job(session_id, TaskStatus::Completed))
        .unwrap();

    let entries = collect_with_jobs(cfg.path(), false);
    let row = entries
        .iter()
        .find(|e| e.session_id == session_id)
        .expect("self row present");
    // Busy transport status wins over the terminal job record.
    assert_eq!(row.state, PsViewState::Working);
}

#[test]
fn all_surfaces_processless_terminal_jobs() {
    let cfg = TempDir::new().unwrap();
    // A job for a session with no live PID row.
    JobStore::new(cfg.path())
        .write(&job("sid-ghost", TaskStatus::Completed))
        .unwrap();

    let default = collect_with_jobs(cfg.path(), /*include_all*/ false);
    assert!(
        default.iter().all(|e| e.session_id != "sid-ghost"),
        "process-less terminal job hidden without --all"
    );

    let all = collect_with_jobs(cfg.path(), /*include_all*/ true);
    let ghost = all
        .iter()
        .find(|e| e.session_id == "sid-ghost")
        .expect("--all surfaces process-less terminal job");
    assert_eq!(ghost.state, PsViewState::Done);
    assert_eq!(ghost.pid, 0);
}
