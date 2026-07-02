use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

const LOCK: &str = ".consolidate-lock";

fn lock_at(dir: &std::path::Path) -> ConsolidateLock {
    ConsolidateLock::new(dir, LOCK)
}

#[test]
fn first_acquire_returns_acquired_with_zero_prior() {
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    match lock.try_acquire() {
        LockOutcome::Acquired(guard) => {
            assert_eq!(guard.prior_mtime_ms(), 0);
            guard.commit();
        }
        other => panic!("expected Acquired, got {other:?}"),
    }
}

#[test]
fn second_acquire_from_same_process_is_reclaimable() {
    // A second acquire from the same process MUST succeed — within-process
    // serialization is the caller's concern, not the lock file's.
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    let first = match lock.try_acquire() {
        LockOutcome::Acquired(g) => {
            g.commit();
            g
        }
        other => panic!("expected first Acquired, got {other:?}"),
    };
    drop(first);
    let second = lock.try_acquire();
    assert!(
        matches!(second, LockOutcome::Acquired(_)),
        "same-process re-acquire must succeed, got {second:?}"
    );
}

#[test]
fn rollback_with_zero_removes_lock() {
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    // Don't commit — Drop rolls back. prior_mtime was 0 → unlink.
    if let LockOutcome::Acquired(g) = lock.try_acquire() {
        drop(g);
    }
    assert!(!temp.path().join(LOCK).exists());
}

#[test]
fn lock_guard_rollback_now_restores_prior_mtime() {
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    let path = temp.path().join(LOCK);
    std::fs::write(&path, "").unwrap();
    let prior = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&path, prior).unwrap();
    match lock.try_acquire() {
        LockOutcome::Acquired(g) => g.rollback_now(),
        other => panic!("expected Acquired, got {other:?}"),
    }
    let mtime = lock.last_consolidated_at().unwrap();
    assert!((mtime - 1_700_000_000_000).abs() < 2_000);
}

#[test]
fn lock_guard_drop_without_commit_rolls_back() {
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    match lock.try_acquire() {
        LockOutcome::Acquired(g) => g.commit(),
        other => panic!("expected first Acquired, got {other:?}"),
    }
    let prior = lock.last_consolidated_at().unwrap();
    // Clear so the next acquire succeeds (same-PID fresh lock is reclaimable
    // but we want a clean prior_mtime=0 path here).
    std::fs::remove_file(temp.path().join(LOCK)).unwrap();
    {
        let g = match lock.try_acquire() {
            LockOutcome::Acquired(g) => g,
            other => panic!("expected second Acquired after rollback, got {other:?}"),
        };
        let cur = lock.last_consolidated_at().unwrap();
        assert!(cur >= prior);
        drop(g);
    }
    // After rollback-on-drop with prior 0, the file is unlinked.
    assert!(lock.last_consolidated_at().is_none());
}

#[cfg(unix)]
#[test]
fn fresh_lock_held_by_live_foreign_process_is_held() {
    // Regression for the double-acquire TOCTOU: reclaimability must be judged
    // from the holder's CURRENT mtime, so a freshly (re-)created lock owned by
    // a live foreign process is never unlinked by a racer that had observed an
    // older, stale lock at the same path.
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    std::fs::write(temp.path().join(LOCK), child.id().to_string()).unwrap();
    let outcome = lock.try_acquire();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        matches!(outcome, LockOutcome::Held),
        "fresh foreign-held lock must not be reclaimed, got {outcome:?}"
    );
}

#[cfg(unix)]
#[test]
fn stale_lock_held_by_live_foreign_process_is_reclaimed_by_age() {
    let temp = tempdir().unwrap();
    let lock = lock_at(temp.path());
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let path = temp.path().join(LOCK);
    std::fs::write(&path, child.id().to_string()).unwrap();
    // Age the lock past HOLDER_STALE_SECS.
    let stale = filetime::FileTime::from_unix_time(1_000_000, 0);
    filetime::set_file_mtime(&path, stale).unwrap();
    let outcome = lock.try_acquire();
    let _ = child.kill();
    let _ = child.wait();
    match outcome {
        LockOutcome::Acquired(g) => g.commit(),
        other => panic!("stale lock must be reclaimable by age, got {other:?}"),
    }
}
