//! Sandbox hot-reload subscriber.
//!
//! Subscribes to [`coco_config::RuntimePublisher`] and re-runs the sandbox
//! adapter on each new [`RuntimeConfig`] snapshot. Settings.json edits flow
//! into the live [`SandboxState`] via [`SandboxState::update_config`] without
//! restarting the session.
//!
//! Closes the gap documented in `docs/coco-rs/audit-gaps.md` Round 13:
//! "Sandbox hot-reload subscriber — wiring is not installed".
//!
//! # Lifecycle
//!
//! - The spawned task holds only a `watch::Receiver`. When the upstream
//!   `RuntimePublisher` (and its `Sender`) is dropped — typically when the
//!   `RuntimeReloader` drops at session end — `rx.changed()` returns `Err`
//!   and the task exits cleanly. Callers do not need a separate cancel
//!   signal.
//!
//! - On every published snapshot, [`reapply_sandbox`] re-runs the adapter
//!   with the same input shape that [`crate::session_runtime::build_sandbox_state`]
//!   uses on initial bootstrap. Failures (rule resolution, etc.) log a
//!   warning and the prior config is retained — a hot-reload error never
//!   kills the running session, even when `sandbox.fail_if_unavailable`
//!   is set (that flag is bootstrap-only by design).

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_config::RuntimeConfig;
use coco_config::RuntimePublisher;
use coco_sandbox::EnforcementLevel;
use coco_sandbox::SandboxConfig;
use coco_sandbox::SandboxState;
use coco_sandbox::adapter::AdapterInputs;
use tokio::task::JoinHandle;

/// Spawn the sandbox hot-reload subscriber task.
///
/// Returns the task's [`JoinHandle`] so callers can hold it for the
/// lifetime of the session (or `await` it during shutdown). The task
/// terminates when the publisher's sender drops.
pub fn spawn_sandbox_reload(
    state: Arc<SandboxState>,
    publisher: &RuntimePublisher,
    cwd: PathBuf,
) -> JoinHandle<()> {
    let mut rx = publisher.subscribe();
    // Mark the current snapshot as seen. `state` was already constructed
    // from this snapshot during initial bootstrap, so the first
    // `rx.changed().await` should fire only on a *new* publish.
    let _initial = rx.borrow_and_update();
    drop(_initial);

    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let snapshot = rx.borrow_and_update().clone();
            if let Err(e) = reapply_sandbox(&state, &snapshot, &cwd) {
                tracing::warn!(
                    error = %e,
                    "sandbox hot-reload failed; keeping prior config"
                );
            } else {
                tracing::debug!("sandbox config hot-reloaded");
            }
        }
        tracing::debug!("sandbox reload subscriber: publisher closed; exiting");
    })
}

/// Apply a new [`RuntimeConfig`] snapshot to the live sandbox state.
///
/// Bootstrap-time gate semantics differ from hot-reload semantics:
/// bootstrap respects `sandbox.fail_if_unavailable` and may abort the
/// session. Hot-reload **never** aborts — if the new snapshot would
/// disable the feature gate or flip to `FullAccess`, we degrade to
/// [`EnforcementLevel::Disabled`] in-place.
fn reapply_sandbox(
    state: &SandboxState,
    runtime: &RuntimeConfig,
    cwd: &Path,
) -> anyhow::Result<()> {
    // Feature gate flip / FullAccess mode → effectively disable enforcement
    // without tearing down the runtime state. Subsequent updates re-enable
    // the same `SandboxState` Arc (no fresh allocation, no platform reinit).
    if !runtime.features.enabled(coco_types::Feature::Sandbox)
        || matches!(runtime.sandbox.mode, coco_types::SandboxMode::FullAccess)
    {
        let mut settings = runtime.sandbox.clone();
        settings.enabled = false;
        state.update_config(
            EnforcementLevel::Disabled,
            settings,
            SandboxConfig {
                enforcement: EnforcementLevel::Disabled,
                ..SandboxConfig::default()
            },
        );
        return Ok(());
    }

    let mut sandbox_settings = runtime.sandbox.clone();
    sandbox_settings.enabled = true;

    let mode = runtime.sandbox.mode;
    let settings_root = runtime
        .paths
        .project_dir
        .clone()
        .unwrap_or_else(|| cwd.to_path_buf());
    let permission_allow_rules: Vec<String> = runtime.settings.merged.permissions.allow.clone();
    let permission_deny_rules: Vec<String> = runtime.settings.merged.permissions.deny.clone();
    let additional_directories: Vec<PathBuf> = runtime
        .settings
        .merged
        .permissions
        .additional_directories
        .iter()
        .map(PathBuf::from)
        .collect();
    let coco_temp_dir = std::env::temp_dir().join("coco");
    let worktree = coco_sandbox::detect_worktree_main_repo(cwd);

    // Per-source rule plumbing — drives the `allow_managed_*_only` gates.
    // Sandbox only consumes allow provenance; deny/ask are handled at
    // the engine config layer (see `permission_rule_loader`).
    let (sourced_allow_rules, _sourced_deny_rules, _sourced_ask_rules) =
        runtime.settings.sourced_permission_rules();
    let sourced_fs_allow_read = runtime.settings.sourced_filesystem_allow_read();

    // Same self-permission deny set as bootstrap — passing `&[]` here silently
    // dropped the S8 protection on the first settings hot-reload, re-opening
    // the sandbox escape mid-session.
    let settings_files = crate::session_runtime::sandbox_settings_deny_paths(&settings_root);

    let inputs = AdapterInputs {
        settings: &sandbox_settings,
        mode,
        settings_root: &settings_root,
        original_cwd: cwd,
        current_cwd: cwd,
        permission_allow_rules: &permission_allow_rules,
        permission_deny_rules: &permission_deny_rules,
        additional_directories: &additional_directories,
        coco_temp_dir: &coco_temp_dir,
        settings_files: &settings_files,
        worktree_main_repo: worktree.as_deref(),
        sourced_permission_allow_rules: Some(&sourced_allow_rules),
        sourced_filesystem_allow_read: Some(&sourced_fs_allow_read),
    };
    let out = coco_sandbox::build_runtime_config(inputs);
    state.update_config(out.enforcement, out.settings, out.config);
    Ok(())
}

#[cfg(test)]
#[path = "sandbox_reload.test.rs"]
mod tests;
