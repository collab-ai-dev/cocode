//! Spawn the skill-change watcher and hot-reload the session's skill
//! catalog + slash-command registry on `.md` edits.
//!
//! The detector scans each debounced burst and emits the pending change;
//! this forwarder runs blocking `ConfigChange(source=Skills)` hooks
//! before mutating the live [`coco_skills::SkillManager`] and rebuilding
//! the slash-command registry.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_skills::watcher::SkillChangeDetector;
use coco_skills::watcher::session_reload_scopes;
use coco_types::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

use crate::session_runtime::SessionHandle;

/// Skill directories watched in every interactive session — the same
/// dirs [`crate::session_runtime::SessionRuntime::reload_plugins`] /
/// `build_session_command_registry` load from:
/// - `<config_home>/skills` — user scope
/// - every `<ancestor>/project config dir/skills` from cwd upward
pub fn default_watch_paths(cwd: &Path, config_home: &Path) -> Vec<PathBuf> {
    session_reload_scopes(config_home, cwd)
        .into_iter()
        .filter(|scope| !matches!(scope, coco_skills::watcher::SkillReloadScope::Managed(_)))
        .map(|scope| scope.path().to_path_buf())
        .collect()
}

/// Spawn the skill-change detector plus a forwarder that rebuilds the
/// slash-command registry and refreshes the TUI command list on each
/// debounced burst.
///
/// Returns the `Arc<SkillChangeDetector>` the caller must hold for the
/// session lifetime (drop = clean shutdown — the wrapped `FileWatcher`
/// and the forwarder task both stop when the last `Arc` drops). Returns
/// `None` when construction fails (logged at `warn`); the session
/// continues without hot-reload rather than aborting.
pub fn spawn(
    session: SessionHandle,
    notify_tx: mpsc::Sender<CoreEvent>,
    cwd: PathBuf,
    config_home: PathBuf,
) -> Option<Arc<SkillChangeDetector>> {
    spawn_current_session(
        session.clone(),
        Arc::new(tokio::sync::RwLock::new(session)),
        notify_tx,
        cwd,
        config_home,
    )
}

/// Spawn the skill-change detector using the swappable current-session owner.
///
/// The filesystem watch roots remain fixed to the TUI project/config roots, but
/// each debounced reload resolves the current [`SessionHandle`] before running
/// hooks or rebuilding slash commands. After `/resume` or `/branch`, skill
/// edits therefore mutate the replacement runtime rather than the startup
/// runtime captured when the watcher was created.
pub fn spawn_current_session(
    initial_session: SessionHandle,
    current_session: Arc<tokio::sync::RwLock<SessionHandle>>,
    notify_tx: mpsc::Sender<CoreEvent>,
    cwd: PathBuf,
    config_home: PathBuf,
) -> Option<Arc<SkillChangeDetector>> {
    let scopes = session_reload_scopes(&config_home, &cwd);
    match SkillChangeDetector::new(initial_session.skill_manager(), scopes) {
        Ok(detector) => {
            let mut rx = detector.subscribe();
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let session = current_session.read().await.clone();
                    let changed_path = event
                        .changed_paths
                        .first()
                        .map(|path| path.to_string_lossy().into_owned());
                    let hook_result = session
                        .run_config_change_hooks(
                            coco_hooks::orchestration::ConfigChangeSource::Skills,
                            changed_path.as_deref(),
                        )
                        .await;
                    if hook_result.is_blocked() {
                        tracing::warn!(
                            path = ?changed_path,
                            "skill reload blocked by ConfigChange hook"
                        );
                        continue;
                    }

                    // Rebuild the live catalog and slash-command registry
                    // from the fresh on-disk skills, then push the refreshed
                    // list to the `/` autocomplete.
                    let session_cwd = session.current_cwd().read().await.clone();
                    let count = session.reload_plugins(&session_cwd).await;
                    tracing::info!(commands = count, "skills changed: command registry rebuilt");
                    let snapshot =
                        crate::session_dialogs::build_available_commands_payload(&session).await;
                    let _ = notify_tx
                        .send(CoreEvent::Tui(TuiOnlyEvent::AvailableCommandsRefreshed {
                            commands: snapshot,
                        }))
                        .await;
                }
            });
            Some(detector)
        }
        Err(err) => {
            tracing::warn!("skill watcher disabled: {err}");
            None
        }
    }
}

#[cfg(test)]
#[path = "skill_watch.test.rs"]
mod tests;
