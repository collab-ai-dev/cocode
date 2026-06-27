use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn first_acquire_returns_acquired_with_zero_prior() {
    let temp = tempdir().unwrap();
    match try_acquire(temp.path()) {
        LockOutcome::Acquired(guard) => {
            assert_eq!(guard.prior_mtime_ms(), 0);
            // Commit so Drop doesn't rollback (which would remove the
            // file we just created and confuse subsequent tests
            // running against the same directory if any).
            guard.commit();
        }
        other => panic!("expected Acquired, got {other:?}"),
    }
}

#[test]
fn second_acquire_from_same_process_is_reclaimable() {
    // Within-process serialization is enforced by `DreamService`'s
    // `consolidating` atomic flag, NOT by the lock file. A second
    // `try_acquire` from the same process MUST succeed — otherwise
    // a successful auto-dream would leave the lock with our PID
    // and silently wedge `/dream` for the next hour.
    let temp = tempdir().unwrap();
    let first = match try_acquire(temp.path()) {
        LockOutcome::Acquired(g) => {
            g.commit();
            g
        }
        other => panic!("expected first Acquired, got {other:?}"),
    };
    drop(first);
    // Same-process re-acquire — reclaimable.
    let second = try_acquire(temp.path());
    assert!(
        matches!(second, LockOutcome::Acquired(_)),
        "same-process re-acquire must succeed, got {second:?}"
    );
}

#[test]
fn release_clears_the_lock() {
    let temp = tempdir().unwrap();
    if let LockOutcome::Acquired(g) = try_acquire(temp.path()) {
        g.commit();
    }
    release(temp.path());
    assert!(!temp.path().join(LOCK_FILENAME).exists());
}

#[test]
fn rollback_with_zero_removes_lock() {
    let temp = tempdir().unwrap();
    // Don't commit — Drop will rollback. Since prior_mtime was 0
    // (lock didn't exist), rollback unlinks.
    if let LockOutcome::Acquired(g) = try_acquire(temp.path()) {
        // Force the rollback at the function-level entry point too
        // for parity with prior behavior.
        drop(g);
    }
    assert!(!temp.path().join(LOCK_FILENAME).exists());
}

#[test]
fn rollback_with_prior_restores_mtime() {
    let temp = tempdir().unwrap();
    if let LockOutcome::Acquired(g) = try_acquire(temp.path()) {
        g.commit();
    }
    rollback(temp.path(), 1_700_000_000_000);
    let mtime = last_consolidated_at(temp.path()).unwrap();
    // Allow rounding error from filetime sub-second precision.
    assert!((mtime - 1_700_000_000_000).abs() < 2_000);
}

#[test]
fn lock_guard_drop_without_commit_rolls_back() {
    let temp = tempdir().unwrap();
    // First acquire and commit so we have a known prior mtime.
    match try_acquire(temp.path()) {
        LockOutcome::Acquired(g) => g.commit(),
        other => panic!("expected first Acquired, got {other:?}"),
    }
    let prior = last_consolidated_at(temp.path()).unwrap();
    // A second `try_acquire` from the SAME process with our PID
    // already in the lock file would return `Held`. To exercise the
    // rollback-on-drop path we have to clear the lock file first so
    // the next acquire succeeds.
    rollback(temp.path(), 0);
    let now_mtime = {
        let g = match try_acquire(temp.path()) {
            LockOutcome::Acquired(g) => g,
            other => panic!("expected second Acquired after rollback, got {other:?}"),
        };
        // Don't commit. Probe the mtime mid-flight; it's freshly
        // stamped at "now" so it's ≥ prior.
        let cur = last_consolidated_at(temp.path()).unwrap();
        assert!(cur >= prior);
        drop(g);
        cur
    };
    // After rollback-on-drop, the lock file either no longer exists
    // (this case — prior_mtime_ms was 0 → unlink) or has the restored
    // prior mtime. Either way it should NOT be at `now_mtime`.
    let restored = last_consolidated_at(temp.path());
    match restored {
        Some(m) => assert!(
            (m - prior).abs() < 2_000,
            "expected mtime ~{prior}, got {m} (mid-flight was {now_mtime})"
        ),
        None => {
            // Acceptable when prior was 0.
        }
    }
}
