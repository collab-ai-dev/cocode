use super::*;

impl SessionRuntime {
    /// Install the MCP handle that every per-turn engine receives via
    /// `wire_engine`. Call this after `SessionRuntime::build` returns
    /// so the bootstrap can wrap a real `McpConnectionManager`.
    pub async fn attach_mcp_handle(&self, handle: coco_tool_runtime::McpHandleRef) {
        let mut slot = self.integration_resources.mcp_handle().write().await;
        *slot = Some(handle);
    }
    /// Snapshot the installed MCP handle. `None` => no handle wired.
    pub async fn current_mcp_handle(&self) -> Option<coco_tool_runtime::McpHandleRef> {
        self.integration_resources.mcp_handle().read().await.clone()
    }
    pub(in crate::session::session_runtime) async fn current_mcp_manager(
        &self,
    ) -> Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>> {
        self.integration_resources
            .mcp_manager()
            .read()
            .await
            .clone()
    }
    /// Install the live `McpConnectionManager` so reload paths can re-register
    /// plugin-contributed MCP servers. Call this after `SessionRuntime::build`
    /// on entry points that own a manager (the AppServer path today).
    pub async fn attach_mcp_manager(
        &self,
        manager: Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>,
    ) {
        let mut slot = self.integration_resources.mcp_manager().write().await;
        *slot = Some(manager);
    }
    /// Current MCP reconnect key. Increments each time
    /// [`Self::reload_plugin_mcp_servers`] changes the registered set.
    pub fn mcp_reconnect_key(&self) -> u64 {
        self.integration_resources
            .mcp_reconnect_key()
            .load(Ordering::Relaxed)
    }
    pub(in crate::session::session_runtime) fn bump_mcp_reconnect_key(&self) {
        self.integration_resources
            .mcp_reconnect_key()
            .fetch_add(1, Ordering::Relaxed);
    }
    /// Install or replace the late-bound LSP handle. Same semantics as
    /// [`Self::attach_mcp_handle`] - slot is read at every
    /// `wire_engine` call so per-turn engines pick up swaps.
    pub async fn attach_lsp_handle(&self, handle: coco_tool_runtime::LspHandleRef) {
        let mut slot = self.integration_resources.lsp_handle().write().await;
        *slot = Some(handle);
    }
    /// Snapshot the installed LSP handle. `None` => no handle wired -
    /// `wire_engine` falls back to `NoOpLspHandle` and `LspTool` hides
    /// from the model.
    pub async fn current_lsp_handle(&self) -> Option<coco_tool_runtime::LspHandleRef> {
        self.integration_resources.lsp_handle().read().await.clone()
    }
}
