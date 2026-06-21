//! Bootstrap helper that wires the keybindings stack into the TUI.
//!
//! Called by `app/cli::tui_runner` after `App::new` and before
//! `app.run()`. Owns the boring plumbing so callers don't have to
//! import `KeybindingHandle`, `install_global`, or fiddle with the
//! watcher's lifetime.
//!
//! Returns the [`KeybindingsWatcher`] so the caller can keep it alive
//! for the TUI's lifetime — drop it on shutdown to stop the
//! background refresh task.

use coco_keybindings::KeybindingsLoadResult;
use coco_keybindings::KeybindingsWatcher;
use coco_keybindings::ValidationIssue;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::keybinding_resolver::KeybindingHandle;

/// Result of [`install_keybindings`]. The caller assigns `handle` into
/// `app.state.ui.kb_handle`, holds `watcher` alive for the TUI
/// lifetime, and plugs `warnings_rx` into
/// [`crate::App::with_keybinding_warnings`].
pub struct KeybindingSetup {
    /// Hold-alive guard. Dropping this stops the watcher's background
    /// task and disables hot reload.
    pub watcher: KeybindingsWatcher,
    /// Receiver for hot-reload validation issues. Each non-empty
    /// vector represents the warnings from a fresh load. Empty
    /// vectors arrive when a previously-broken file becomes clean —
    /// the App ignores those (no toasts) but the receiver still
    /// drains them so the channel doesn't fill.
    pub warnings_rx: mpsc::Receiver<Vec<ValidationIssue>>,
    /// Initial load result — startup warnings (if any) should be
    /// surfaced by the caller via [`AppState::ui::add_toast`] once,
    /// before the App starts; subsequent reloads flow through
    /// `warnings_rx`.
    pub initial: KeybindingsLoadResult,
    /// Watcher-backed handle. Caller installs it into
    /// `app.state.ui.kb_handle` so the in-memory resolver hot-reloads
    /// alongside the file.
    pub handle: KeybindingHandle,
}

/// Build the keybindings watcher and a watcher-backed handle, plus
/// the warnings channel for toast surfacing.
///
/// Caller wiring:
/// 1. `app.state.ui.kb_handle = setup.handle;`
/// 2. `app = app.with_keybinding_warnings(setup.warnings_rx);`
/// 3. `let _guard = setup.watcher;` (hold for TUI lifetime)
pub async fn install_keybindings() -> KeybindingSetup {
    let watcher = KeybindingsWatcher::watch_default().await;
    let initial = watcher.current();

    // Build the handle from the initial state. The handle takes a
    // subscription internally so its in-memory resolver rebuilds on
    // each hot-reload event — no global needed, the caller stores it
    // in `AppState.ui`.
    let handle = KeybindingHandle::with_watcher(initial.clone(), &watcher);

    // Spawn a second subscription that forwards each reload's
    // warnings into a channel the App reads in its select! loop.
    let (warn_tx, warn_rx) = mpsc::channel::<Vec<ValidationIssue>>(8);
    let mut rx = watcher.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(result) => {
                    if warn_tx.send(result.warnings).await.is_err() {
                        // App dropped its receiver — TUI is shutting
                        // down, exit the forwarder.
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    KeybindingSetup {
        watcher,
        warnings_rx: warn_rx,
        initial,
        handle,
    }
}
