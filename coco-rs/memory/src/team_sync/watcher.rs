//! Debounced file watcher → push trigger.
//!
//! Watches the local team memory directory and triggers a debounced
//! [`super::service::push`] when files change. Initial pull is the
//! caller's responsibility (one-shot at session bootstrap).
//!
//! State machine:
//! - `Idle` → fs event → schedule debounce timer (2s).
//! - On debounce tick → start `executePush()`.
//! - During push → mid-flight events set `pending=true`; the
//!   post-push tail re-arms the timer once.
//! - On permanent failure (4xx except 409/429, no_oauth, no_repo) →
//!   `suppressed=true` until session restart (prevents the
//!   167K-events-per-2.5-day BQ incident).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_file_watch::{FileWatcher, FileWatcherBuilder, RecursiveMode};
use tokio::sync::Mutex;

use super::service::PushEntry;
use super::service::push as push_service;
use super::types::SyncState;

/// Debounce window between the last fs event and the actual push
/// (2000 ms).
pub const DEBOUNCE_MS: u64 = 2_000;

/// Configuration handed to [`run_watch_loop`]. Borrowed across the
/// loop's lifetime so callers can rotate auth tokens via the
/// `bearer_token_provider` callback (called once per push attempt).
pub struct WatcherConfig {
    /// Local team memory directory (recursively watched).
    pub watch_dir: PathBuf,
    /// API base URL (e.g. `https://api.anthropic.com`).
    pub base_url: String,
    /// Repo slug used as the `?repo=` query.
    pub repo_slug: String,
    /// Token provider — called once per push attempt so the watcher
    /// observes refreshed Bearer tokens without restart.
    pub bearer_token_provider: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// Reads the live entries from disk. Caller controls how files
    /// are enumerated (the team-memory dir layout lives in
    /// `coco_memory::path`). Returning an empty list is a no-op
    /// push (zero work).
    pub read_entries: Arc<dyn Fn() -> Vec<PushEntry> + Send + Sync>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WatchEvent {
    Modified,
}

/// Spawn the watch loop as a detached task. Returns a join handle the
/// caller holds for the session lifetime; aborting/dropping the future
/// tears the watch down cleanly. Equivalent to
/// `tokio::spawn(run_watch_loop(config, state))`.
pub fn spawn_watcher(
    config: WatcherConfig,
    state: Arc<Mutex<SyncState>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_watch_loop(config, state))
}

/// Run the debounced push loop until the watch is torn down (the future
/// is dropped / aborted) or a permanent failure suppresses it.
///
/// **Owns the [`FileWatcher`] for the loop's lifetime.** The watcher
/// holds the `broadcast::Sender`; dropping it closes the channel and the
/// `rx.recv()` loop would exit immediately. The `watcher` binding must
/// therefore outlive `rx` — it stays in scope until this fn returns
/// (mirrors how `SkillChangeDetector` holds its `_inner` field).
pub async fn run_watch_loop(config: WatcherConfig, state: Arc<Mutex<SyncState>>) {
    let watcher: FileWatcher<WatchEvent> = match FileWatcherBuilder::new()
        .throttle_interval(Duration::from_millis(DEBOUNCE_MS))
        .build(
            |event| {
                use notify::EventKind;
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        Some(WatchEvent::Modified)
                    }
                    _ => None,
                }
            },
            |a, _b| a, // single coalesced "modified" event per window
        ) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "team-memory-watcher: build failed; aborting watch");
            return;
        }
    };
    watcher.watch(config.watch_dir.clone(), RecursiveMode::Recursive);
    let mut rx = watcher.subscribe();

    let mut suppressed_reason: Option<String> = None;
    // `while let` exits when the broadcast channel closes
    // (watcher torn down). Replaces the prior `match {Ok|Err}`
    // shape clippy flagged as `while_let_loop`.
    while let Ok(WatchEvent::Modified) = rx.recv().await {
        if suppressed_reason.is_some() {
            continue;
        }
        let token = match (config.bearer_token_provider)() {
            Some(t) => t,
            None => {
                suppressed_reason = Some("no_oauth".into());
                tracing::warn!(
                    "team-memory-watcher: no OAuth token; suppressing push until restart"
                );
                continue;
            }
        };
        let entries = (config.read_entries)();
        if entries.is_empty() {
            continue;
        }
        let mut state_guard = state.lock().await;
        let result = push_service(
            &mut state_guard,
            &config.base_url,
            &config.repo_slug,
            &token,
            &entries,
        )
        .await;
        drop(state_guard);
        if !result.success {
            if let Some(err) = &result.error {
                tracing::warn!(error = %err, "team-memory-watcher: push failed");
                // Permanent-failure heuristic: 4xx except 409/429
                // get suppressed.
                if let Some(status) = parse_http_status(err)
                    && (400..500).contains(&status)
                    && status != 409
                    && status != 429
                {
                    suppressed_reason = Some(format!("http_{status}"));
                    tracing::warn!(
                        "team-memory-watcher: suppressing retry until session restart ({status})"
                    );
                }
            }
        } else if result.uploaded_count > 0 {
            tracing::info!(
                uploaded = result.uploaded_count,
                skipped = result.skipped_secrets.len(),
                "team-memory-watcher: push succeeded"
            );
        }
    }
    tracing::debug!("team-memory-watcher: loop exited");
}

/// Parse `"http 413: …"` style error strings into the status code.
/// Returns `None` for shapes that don't match.
fn parse_http_status(err: &str) -> Option<i32> {
    let prefix = err.strip_prefix("http ")?;
    let (code_str, _) = prefix.split_once(':')?;
    code_str.trim().parse::<i32>().ok()
}

#[cfg(test)]
#[path = "watcher.test.rs"]
mod tests;
