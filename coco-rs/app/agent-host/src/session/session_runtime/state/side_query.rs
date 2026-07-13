use super::*;

impl SessionRuntime {
    pub async fn flush_session_usage_snapshot(&self) {
        self.turn_resources
            .usage_accounting()
            .flush_snapshot()
            .await;
    }
    pub async fn session_usage_snapshot(&self) -> coco_types::SessionUsageSnapshot {
        self.turn_resources.usage_accounting().snapshot().await
    }
    pub fn side_query(&self) -> coco_tool_runtime::SideQueryHandle {
        self.turn_resources.side_query()
    }
    pub(crate) fn usage_accounting(&self) -> coco_query::usage_accounting::UsageAccounting {
        self.turn_resources.usage_accounting()
    }
    pub async fn install_side_query_event_tx(&self, event_tx: mpsc::Sender<coco_query::CoreEvent>) {
        self.turn_resources
            .usage_accounting()
            .install_event_tx(event_tx)
            .await;
    }
    /// Generate the on-demand LLM risk explanation for a permission prompt.
    /// Runs the explainer via the session `SideQuery` handle, gated on
    /// `permission_explainer_enabled` (default-on) and bounded by a timeout.
    /// Graceful-degrades to `None` when the setting is off, the side query
    /// errors, or the timeout elapses. The single home for the explainer call
    /// - `TuiPermissionBridge::explain_risk` and the tui_runner Ctrl+E path
    /// both delegate here.
    pub async fn explain_permission_risk(
        &self,
        params: coco_permissions::ExplainerParams<'_>,
    ) -> Option<coco_types::PermissionExplanation> {
        if !self
            .runtime_config()
            .settings
            .merged
            .permissions
            .explainer_enabled()
        {
            return None;
        }
        let handle = self.side_query();
        let fut =
            coco_permissions::generate_permission_explanation(params, move |req| async move {
                handle.query(req).await.map_err(|e| e.to_string())
            });
        // Bound the timeout so a slow/hung side query can't pin the explainer panel.
        tokio::time::timeout(std::time::Duration::from_secs(8), fut)
            .await
            .unwrap_or_default()
    }
}
