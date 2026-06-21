//! Bridge: `coco_lsp::LspServerManager` → `coco_tool_runtime::LspHandle`.
//!
//! Lives in `app/cli` because it sits at the seam where both crates
//! are deps. Mirrors [`crate::mcp_handle_adapter`] structurally — a
//! thin adapter that wraps the service-layer manager and exposes the
//! tool-runtime trait contract.
//!
//! The trait indirection keeps `coco-tools` independent of `coco-lsp`
//! and improves testability.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use coco_lsp::DiagnosticsStore;
use coco_lsp::LspServerManager;
use coco_tool_runtime::LspHandle;
use serde_json::Value;
use tracing::debug;
use tracing::info;
use tracing::warn;

/// Adapter that wraps a shared `Arc<LspServerManager>` and implements
/// [`LspHandle`].
///
/// `diagnostics` is the same `DiagnosticsStore` registered with the
/// manager (see `LspServerManager::diagnostics()`). It is borrowed so
/// `notify_save` can clear the cross-turn delivered-LRU for the saved
/// file before the LSP `didSave` notification.
pub struct LspManagerAdapter {
    manager: Arc<LspServerManager>,
    diagnostics: Arc<DiagnosticsStore>,
    /// Tracks whether at least one configured server is in a non-failed
    /// state. Set to `true` at construction (lazy-spawn path: we trust
    /// the config until proven otherwise — `stopped` / `starting` /
    /// `running` states are all considered active). Refined by
    /// [`Self::prewarm`] — after eager init we know exactly how many
    /// servers actually started, so we clear the flag if **every**
    /// spawn attempt failed.
    has_active: Arc<AtomicBool>,
}

impl LspManagerAdapter {
    pub fn new(manager: Arc<LspServerManager>) -> Self {
        let diagnostics = manager.diagnostics().clone();
        let has_active = Arc::new(AtomicBool::new(manager.has_configured_servers()));
        Self {
            manager,
            diagnostics,
            has_active,
        }
    }

    /// Eagerly spawn LSP servers for every configured extension, anchored
    /// at `project_root`. Runs at session bootstrap so the `LSPTool.isEnabled()`
    /// gate reads accurate running-state by the time the model emits its
    /// first turn.
    ///
    /// Without this, the lazy-spawn path defers the first connect to the
    /// first `LspTool` call — fine for correctness, but the tool would
    /// briefly appear "connected" via `has_configured_servers` then
    /// produce a `ServerNotInstalled` error if the binary is missing.
    pub async fn prewarm(&self, project_root: &Path) {
        let extensions: Vec<String> = self.manager.all_supported_extensions().await;
        if extensions.is_empty() {
            self.has_active.store(false, Ordering::Relaxed);
            return;
        }
        let ext_refs: Vec<&str> = extensions.iter().map(String::as_str).collect();
        let started = self.manager.prewarm(&ext_refs, project_root).await;
        info!(
            target: "coco_cli::lsp",
            configured = extensions.len(),
            started = started.len(),
            root = %project_root.display(),
            "LSP prewarm complete"
        );
        // If prewarm started ZERO servers (all binaries missing / all
        // failed), flip the gate off so `LspTool` hides cleanly.
        self.has_active
            .store(!started.is_empty(), Ordering::Relaxed);
    }

    /// Merge plugin-contributed LSP servers into the live manager via its
    /// `merge_config` seam. Call **before** [`Self::prewarm`] so plugin
    /// servers are eagerly spawned alongside the disk-configured ones.
    pub async fn merge_plugin_servers(&self, servers: coco_lsp::LspServersConfig) {
        if servers.servers.is_empty() {
            return;
        }
        self.manager.merge_config(servers).await;
        self.has_active
            .store(self.manager.has_configured_servers(), Ordering::Relaxed);
    }

    /// Re-read `lsp_servers.json` from disk **and** re-prewarm.
    /// Intended for the future settings-watcher hook (`SettingsWatcher`
    /// detects a `.coco/lsp_servers.json` change) or a manual `/lsp
    /// reload` slash command. Idempotent: a cached client survives the
    /// prewarm cache-check; only new entries spawn. Plugin-contributed
    /// servers (dropped by the disk reload) are re-merged.
    pub async fn reload_and_prewarm(&self, project_root: &Path) {
        self.manager.reload_config().await;
        let plugins = coco_plugins::load_enabled_plugins(
            &coco_config::global_config::config_home(),
            project_root,
        );
        let refs: Vec<&coco_plugins::loader::LoadedPluginV2> = plugins.iter().collect();
        self.merge_plugin_servers(coco_plugins::lsp_bridge::extract_lsp_servers_from_plugins(
            &refs,
        ))
        .await;
        self.prewarm(project_root).await;
    }
}

#[async_trait::async_trait]
impl LspHandle for LspManagerAdapter {
    fn is_connected(&self) -> bool {
        // Two gates: the config-level check guards against re-enabling
        // after a `reload_config` removes the last server; the
        // running-state atomic guards against the "configured but all
        // spawns failed" path (only meaningful after `prewarm`).
        self.manager.has_configured_servers() && self.has_active.load(Ordering::Relaxed)
    }

    async fn send_request(
        &self,
        file_path: &Path,
        method: &str,
        params: Value,
    ) -> Result<Value, coco_error::BoxedError> {
        let client = self.manager.get_client(file_path).await.map_err(|e| {
            Box::new(coco_error::PlainError::new(
                format!("no LSP server for {}: {e}", file_path.display()),
                coco_error::StatusCode::Internal,
            )) as coco_error::BoxedError
        })?;

        // Ensure the document is opened on the server before any
        // `textDocument/*` request. `workspace/symbol` doesn't strictly
        // need an open document, but syncing the anchor file is cheap
        // and keeps the routing predictable.
        client.sync_file(file_path).await.map_err(|e| {
            Box::new(coco_error::PlainError::new(
                format!("LSP sync_file failed for {}: {e}", file_path.display()),
                coco_error::StatusCode::Internal,
            )) as coco_error::BoxedError
        })?;

        debug!(target: "coco_cli::lsp", method, path = %file_path.display(), "dispatching LSP request");

        client.send_raw_request(method, params).await.map_err(|e| {
            Box::new(coco_error::PlainError::new(
                format!("LSP {method} failed: {e}"),
                coco_error::StatusCode::Internal,
            )) as coco_error::BoxedError
        })
    }

    async fn notify_save(&self, file_path: &Path) {
        // Clear cross-turn dedup *before* the notify so re-published
        // diagnostics for the saved file are not suppressed.
        let path_buf: PathBuf = file_path.to_path_buf();
        self.diagnostics.clear_delivered_for_file(&path_buf).await;

        let client = match self.manager.get_client(file_path).await {
            Ok(c) => c,
            Err(err) => {
                // No server for this extension is normal — skip silently.
                debug!(target: "coco_cli::lsp", path = %file_path.display(), %err, "notify_save skipped (no server)");
                return;
            }
        };

        if let Err(err) = client.notify_save(file_path).await {
            warn!(target: "coco_cli::lsp", path = %file_path.display(), %err, "LSP didSave failed");
        }
    }

    async fn shutdown_for_root(&self, root_path: &Path) {
        debug!(
            target: "coco_cli::lsp",
            root = %root_path.display(),
            "shutting down LSP servers for root"
        );
        self.manager.shutdown_for_root(root_path).await;
    }

    async fn reload(&self, project_root: &Path) {
        // `/reload-plugins`: re-read the disk config + re-merge plugin
        // LSP servers + re-prewarm.
        self.reload_and_prewarm(project_root).await;
    }
}

#[cfg(test)]
#[path = "lsp_handle_adapter.test.rs"]
mod tests;
