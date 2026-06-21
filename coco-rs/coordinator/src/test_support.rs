//! Shared test-only helpers for the coordinator crate.
//!
//! Coordinator tests mutate **process-global** state — `COCO_TEAMS_DIR` (the
//! team/mailbox file root) and the `COCO_*` teammate-identity env vars
//! (`COCO_AGENT_ID` / `COCO_TEAM_NAME` / …). nextest runs each test in its own
//! process so these are perfectly isolated there, but under threaded
//! `cargo test` every test in the crate shares one process: a test setting
//! `COCO_TEAM_NAME` would be observed by a concurrent `get_team_name()` reader,
//! and two tests both pointing `COCO_TEAMS_DIR` at their own tempdir would
//! clobber each other.
//!
//! [`ENV_LOCK`] is the single crate-wide serialization point. Every test that
//! reads or writes process-global environment holds it for its whole body, so
//! no two such tests run concurrently and none observes another's transient
//! env.

use std::sync::Arc;
use std::sync::LazyLock;

use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;

/// Crate-wide serialization for any test touching process-global environment.
/// Hold the returned guard for the entire test body.
pub(crate) static ENV_LOCK: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

/// Acquire the shared env lock. For tests that only need mutual exclusion with
/// other env-touching tests — e.g. they *read* identity env vars and assert on
/// the ambient value — without setting `COCO_TEAMS_DIR` themselves.
pub(crate) async fn lock_env() -> OwnedMutexGuard<()> {
    ENV_LOCK.clone().lock_owned().await
}

/// RAII guard pointing `COCO_TEAMS_DIR` at a fresh per-test tempdir.
///
/// Team creation runs directory-WIDE probes (`unique_team_name` stats sibling
/// dirs; cleanup/discovery `read_dir` the whole `teams/` tree). Without
/// isolation all team tests share the real `~/.coco/teams/` tree, so those
/// probes race sibling tests' `create_dir_all` / `remove_dir_all` /
/// half-written `config.json`. Pointing each test at its own empty tree
/// confines every probe to that one test. Holds [`ENV_LOCK`] for its lifetime
/// and restores the env on drop.
pub(crate) struct IsolatedTeamsDir {
    _tmp: tempfile::TempDir,
    _lock: OwnedMutexGuard<()>,
}

impl Drop for IsolatedTeamsDir {
    fn drop(&mut self) {
        // SAFETY: serialized via the held `ENV_LOCK` guard.
        unsafe { std::env::remove_var("COCO_TEAMS_DIR") };
    }
}

pub(crate) async fn isolate_teams_dir() -> IsolatedTeamsDir {
    let lock = lock_env().await;
    let tmp = tempfile::tempdir().unwrap();
    let teams = tmp.path().join("teams");
    std::fs::create_dir_all(&teams).unwrap();
    // SAFETY: serialized via the held `ENV_LOCK` guard; nextest isolates per
    // process.
    unsafe { std::env::set_var("COCO_TEAMS_DIR", &teams) };
    IsolatedTeamsDir {
        _tmp: tmp,
        _lock: lock,
    }
}
