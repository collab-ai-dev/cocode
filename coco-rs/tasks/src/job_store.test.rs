//! Tests for the durable background-job record store.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use coco_session::SessionKind;
use coco_types::TaskStatus;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn sample_job(id: &str, status: TaskStatus) -> JobState {
    let now = now_ms();
    JobState {
        id: id.to_string(),
        session_id: format!("session-{id}"),
        cwd: PathBuf::from("/work"),
        kind: SessionKind::DaemonWorker,
        created_at: now,
        updated_at: now,
        status,
        name: Some("nightly-eval".into()),
        intent: Some("run the eval suite".into()),
        error: None,
    }
}

#[test]
fn write_then_read_round_trips() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    let job = sample_job("job-1", TaskStatus::Running);

    store.write(&job).unwrap();
    let read_back = store.read("job-1").unwrap().expect("job must exist");
    assert_eq!(read_back, job);
}

#[test]
fn read_missing_returns_none() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    assert!(store.read("nope").unwrap().is_none());
}

#[test]
fn write_creates_dir_with_0700() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    store
        .write(&sample_job("job-x", TaskStatus::Completed))
        .unwrap();

    let dir = cfg.path().join("bg-jobs");
    assert!(dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn list_returns_all_written_jobs() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    store
        .write(&sample_job("a", TaskStatus::Completed))
        .unwrap();
    store.write(&sample_job("b", TaskStatus::Failed)).unwrap();
    store.write(&sample_job("c", TaskStatus::Killed)).unwrap();

    let mut ids: Vec<String> = store.list().unwrap().into_iter().map(|j| j.id).collect();
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "b".into(), "c".into()]);
}

#[test]
fn list_on_missing_dir_is_empty() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn list_skips_non_json_and_dotfiles() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    store
        .write(&sample_job("real", TaskStatus::Completed))
        .unwrap();

    let dir = cfg.path().join("bg-jobs");
    std::fs::write(dir.join("notes.md"), "ignored").unwrap();
    std::fs::write(dir.join(".hidden.json"), "{}").unwrap();

    let ids: Vec<String> = store.list().unwrap().into_iter().map(|j| j.id).collect();
    assert_eq!(ids, vec!["real".to_string()]);
}

#[test]
fn remove_deletes_and_is_idempotent() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    store
        .write(&sample_job("gone", TaskStatus::Failed))
        .unwrap();
    assert!(store.read("gone").unwrap().is_some());

    store.remove("gone").unwrap();
    assert!(store.read("gone").unwrap().is_none());
    // Second remove is a no-op (ENOENT → Ok).
    store.remove("gone").unwrap();
}

#[test]
fn error_field_survives_round_trip() {
    let cfg = TempDir::new().unwrap();
    let store = JobStore::new(cfg.path());
    let mut job = sample_job("err", TaskStatus::Failed);
    job.error = Some("boom".into());
    store.write(&job).unwrap();
    let back = store.read("err").unwrap().unwrap();
    assert_eq!(back.error.as_deref(), Some("boom"));
    assert_eq!(back.status, TaskStatus::Failed);
}
