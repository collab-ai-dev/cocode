use super::*;

impl SessionHandle {
    /// Evaluate the deterministic portion of one tool permission request using
    /// the session's live rules and tool context. The query-layer result names
    /// every dynamic stage it intentionally skips.
    pub async fn probe_static_permission(
        &self,
        tool_name: &str,
        input: serde_json::Value,
    ) -> coco_types::StaticPermissionProbeResult {
        self.build_engine(tokio_util::sync::CancellationToken::new())
            .await
            .probe_static_permission(tool_name, input)
            .await
    }

    pub async fn build_engine(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> coco_query::QueryEngine {
        self.runtime.build_engine(cancel).await
    }

    pub async fn build_turn_engine(
        &self,
        request: super::SessionTurnEngineConfigRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> super::SessionTurnEngine {
        self.runtime.build_turn_engine(request, cancel).await
    }

    pub async fn run_manual_compact(
        &self,
        request: coco_query::ManualCompactRequest,
        event_tx: Option<tokio::sync::mpsc::Sender<coco_event_types::CoreEvent>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> coco_compact::CompactOutcome {
        self.runtime
            .run_manual_compact(request, event_tx, cancel)
            .await
    }

    pub async fn build_engine_from_config(
        &self,
        config: coco_query::QueryEngineConfig,
        session_id: SessionId,
        cancel: tokio_util::sync::CancellationToken,
        app_state_override: Option<Arc<tokio::sync::RwLock<coco_types::ToolAppState>>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_engine_from_config(config, session_id, cancel, app_state_override)
            .await
    }

    pub(crate) async fn build_fork_engine_from_config(
        &self,
        config: coco_query::QueryEngineConfig,
        session_id: SessionId,
        cancel: tokio_util::sync::CancellationToken,
        app_state_override: Option<Arc<tokio::sync::RwLock<coco_types::ToolAppState>>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_fork_engine_from_config(config, session_id, cancel, app_state_override)
            .await
    }

    pub(crate) async fn build_engine_from_config_with_registries(
        &self,
        config: coco_query::QueryEngineConfig,
        session_id: SessionId,
        cancel: tokio_util::sync::CancellationToken,
        tools: Arc<coco_tool_runtime::ToolRegistry>,
        hooks: Option<Arc<coco_hooks::HookRegistry>>,
    ) -> coco_query::QueryEngine {
        self.runtime
            .build_engine_from_config_with_registries(config, session_id, cancel, tools, hooks)
            .await
    }

    pub async fn analyze_main_context(
        &self,
    ) -> coco_query::context_analysis::Result<coco_query::context_analysis::ContextUsageReport>
    {
        self.runtime.analyze_main_context().await
    }
}
