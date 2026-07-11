//! Wire team-memory server sync into the interactive session.
//!
//! On session start it pulls the server's team memory into the local team
//! dir, then runs a debounced watcher that pushes local edits.
//!
//! Activation is gated on three runtime conditions and the bootstrap
//! **no-ops cleanly** when any is absent:
//! - `MemoryConfig.team_memory_enabled` (the sub-toggle),
//! - a resolvable `owner/repo` slug from the git `origin` remote, and
//! - claude.ai OAuth tokens; without a token the initial pull is skipped
//!   and the watcher suppresses, so a logged-out session does zero sync
//!   network work.
//!
//! The sync endpoint is Anthropic-first-party only (`/api/claude_code/
//! team_memory`): the base URL defaults to the Anthropic API base,
//! overridable via `COCO_TEAM_MEMORY_SYNC_URL`. It is wired on the
//! interactive (TUI) path only — scripted `-p` / SDK runs do no
//! background sync.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_config::RuntimeConfig;
use coco_memory::team_sync;
use tokio::sync::Mutex;

/// Default Anthropic API base for the sync endpoint.
const DEFAULT_SYNC_BASE_URL: &str = "https://api.anthropic.com";

/// `COCO_TEAM_MEMORY_SYNC_URL` override, else the Anthropic API base.
fn resolve_base_url() -> String {
    coco_config::env::var(coco_config::env::EnvKey::CocoTeamMemorySyncUrl)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SYNC_BASE_URL.to_string())
}

/// Bearer-token provider: the claude.ai OAuth access token, or `None`
/// when no OAuth login is present (API-key-only or logged-out). Called
/// once per push so the watcher observes refreshed tokens without a
/// restart.
fn make_bearer_provider(config_home: PathBuf) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
    Arc::new(move || {
        let opts = coco_inference::auth::AuthResolveOptions {
            config_dir: Some(config_home.clone()),
            ..Default::default()
        };
        match coco_inference::auth::resolve_auth(&opts) {
            Some(coco_inference::auth::AuthMethod::OAuth(tokens)) => Some(tokens.access_token),
            _ => None,
        }
    })
}

/// Enumerate the live `.md` entries under `team_dir` into [`PushEntry`]s
/// keyed by their path relative to the dir. Secret pre-scanning happens
/// inside `push`, so this only supplies the raw contents.
fn make_read_entries(
    team_dir: PathBuf,
) -> Arc<dyn Fn() -> Vec<team_sync::PushEntry> + Send + Sync> {
    Arc::new(move || collect_md_entries(&team_dir))
}

fn collect_md_entries(team_dir: &Path) -> Vec<team_sync::PushEntry> {
    let mut out = Vec::new();
    collect_md_recursive(team_dir, team_dir, &mut out);
    out
}

fn collect_md_recursive(root: &Path, dir: &Path, out: &mut Vec<team_sync::PushEntry>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md_recursive(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "md")
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(team_sync::PushEntry {
                path: rel.to_string_lossy().replace('\\', "/"),
                content,
            });
        }
    }
}

/// Spawn the team-memory pull-then-watch coordinator if all gates pass.
///
/// Fire-and-forget on the interactive path: the coordinator task owns the
/// watcher for the session and is torn down on process exit (matching the
/// official-marketplace auto-install). No-ops when team memory is
/// disabled or the repo has no `origin` slug.
pub fn bootstrap(runtime_config: &RuntimeConfig, cwd: PathBuf, config_home: PathBuf) {
    if !runtime_config.memory.team_memory_enabled {
        return;
    }
    let project_paths = crate::paths::project_paths(&cwd);
    let team_dir = project_paths.team_memory_dir();
    let base_url = resolve_base_url();
    let bearer = make_bearer_provider(config_home);
    let read_entries = make_read_entries(team_dir.clone());

    tokio::spawn(async move {
        let Some(repo_slug) = coco_git::github_origin_slug(&cwd).await else {
            tracing::debug!("team-memory-sync: no origin slug; sync disabled for this session");
            return;
        };
        let state = Arc::new(Mutex::new(team_sync::SyncState::default()));

        // Initial pull → apply BEFORE the watcher starts so its own disk
        // writes don't trigger a spurious push.
        if let Some(token) = (bearer)() {
            let etag = state.lock().await.last_known_checksum.clone();
            let result = {
                let mut st = state.lock().await;
                team_sync::pull(&mut st, &base_url, &repo_slug, &token, etag.as_deref()).await
            };
            if result.success {
                if let Some(data) = result.data {
                    team_sync::apply_pulled_content(&team_dir, &data.content).await;
                    tracing::info!(repo = %repo_slug, "team-memory-sync: initial pull applied");
                }
            } else if let Some(err) = &result.error {
                tracing::warn!(error = %err, "team-memory-sync: initial pull failed");
            }
        } else {
            tracing::debug!(
                "team-memory-sync: no OAuth token; pull skipped (watcher will suppress)"
            );
        }

        // Then run the debounced push watcher for the session lifetime.
        let config = team_sync::WatcherConfig {
            watch_dir: team_dir,
            base_url,
            repo_slug,
            bearer_token_provider: bearer,
            read_entries,
        };
        team_sync::run_watch_loop(config, state).await;
    });
}

#[cfg(test)]
#[path = "team_memory_sync.test.rs"]
mod tests;
