//! LSP handle trait — abstraction for LSP operations from tools.
//!
//! Same pattern as [`McpHandle`](crate::mcp_handle): trait defined here,
//! implementation in `app/cli/src/lsp_handle_adapter.rs` (wraps
//! `Arc<coco_lsp::LspServerManager>`), injected via `ToolUseContext`.
//!
//! `sendRequest()` — tools call the JSON-RPC method by name and parse the
//! raw response. The trait is intentionally thin: every typed shape lives
//! in the tool layer (`core/tools/src/tools/lsp.rs`) so this crate stays
//! free of `lsp_types`.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

/// Trait for LSP operations exposed to tools.
///
/// Wraps `coco_lsp::LspServerManager` — tools call this instead of
/// depending on `coco-lsp` directly.
#[async_trait::async_trait]
pub trait LspHandle: Send + Sync {
    /// Whether at least one LSP server is configured **and** spawnable for the
    /// current workspace. Used by `Tool::is_enabled` to filter the `LSPTool`
    /// out of the model's tool list when there are no servers.
    ///
    /// Synchronous because `Tool::is_enabled` is sync. Implementations should
    /// back this with an `AtomicBool` (see
    /// `LspServerManager::has_configured_servers`).
    fn is_connected(&self) -> bool;

    /// Open / sync `file_path` in the server, then dispatch a raw JSON-RPC
    /// request. The handle is responsible for routing to the right server
    /// (by file extension), opening the document with `textDocument/didOpen`
    /// if not already tracked, and forwarding `params` verbatim.
    ///
    /// `file_path` is always required, even for `workspace/symbol`, where
    /// it acts as the *anchor* for server selection — the workspace-symbol
    /// schema still requires `filePath`.
    ///
    /// Returns the server's `result` (may be `Value::Null` when the server
    /// reports "no result"). Errors are returned for: no server for this
    /// extension, server unhealthy, protocol error, file too large, IO.
    async fn send_request(
        &self,
        file_path: &Path,
        method: &str,
        params: Value,
    ) -> Result<Value, coco_error::BoxedError>;

    /// Best-effort `textDocument/didSave` notification. Called by file
    /// mutation tools (Write / Edit / NotebookEdit) AFTER successful
    /// disk write so the LSP server re-indexes and emits fresh
    /// diagnostics. Errors are silently swallowed — failing to notify
    /// must never fail the user's tool call.
    ///
    /// The Rust handle implementation is responsible for both — clearing
    /// the delivered-diagnostics LRU here so re-published diagnostics
    /// for the saved file are not suppressed by cross-turn dedup.
    async fn notify_save(&self, file_path: &Path);

    /// Proactively tear down every LSP server rooted at `root_path`.
    /// Called by `ExitWorktreeTool` AFTER `git worktree remove`
    /// succeeded so cached `(server_id, worktree_root)` clients are
    /// released even if the directory entry lingers on disk (e.g.
    /// process cwd was inside the worktree at remove time).
    ///
    /// Best-effort: no-op on `NoOpLspHandle`; errors logged + swallowed
    /// on the real adapter. Without this hook the cleanup path is
    /// lazy — the next `get_client` call after deletion would notice
    /// `key.1.exists() == false` and dispatch
    /// `shutdown_servers_for_root` — but that next call may never
    /// arrive in the session, leaving the LSP server zombie until
    /// session exit.
    async fn shutdown_for_root(&self, root_path: &Path);

    /// Re-read the on-disk LSP config and re-merge plugin-contributed servers,
    /// then re-prewarm. Called from `/reload-plugins` so newly enabled/disabled
    /// plugin LSP servers take effect.
    /// Default no-op — `NoOpLspHandle` and sessions without LSP inherit it.
    async fn reload(&self, _project_root: &Path) {}
}

pub type LspHandleRef = Arc<dyn LspHandle>;

/// No-op implementation for contexts without LSP (tests, subagents,
/// sessions that have `Feature::Lsp` off, or sessions with no
/// configured language server).
///
/// `is_connected()` returns `false` — combined with
/// `LspTool::is_enabled`, this hides the tool from the model's tool
/// list. The async methods return an `Internal` error so any tool that
/// somehow reaches `execute()` despite the gate fails cleanly with a
/// model-visible message instead of panicking.
#[derive(Debug, Clone, Default)]
pub struct NoOpLspHandle;

#[async_trait::async_trait]
impl LspHandle for NoOpLspHandle {
    fn is_connected(&self) -> bool {
        false
    }

    async fn send_request(
        &self,
        _: &Path,
        _: &str,
        _: Value,
    ) -> Result<Value, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "LSP is not configured for this session",
            coco_error::StatusCode::Internal,
        )))
    }

    async fn notify_save(&self, _: &Path) {}

    async fn shutdown_for_root(&self, _: &Path) {}
}

#[cfg(test)]
#[path = "lsp_handle.test.rs"]
mod tests;
