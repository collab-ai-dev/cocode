use super::{
    SessionAgentCatalogResources, SessionCatalogResources, SessionCommandResources,
    SessionConfigResources, SessionEngineConfigResources, SessionEngineStateResources,
    SessionExecutionResources, SessionHandleResources, SessionHistoryResources,
    SessionHookResources, SessionIntegrationResources, SessionLifecycleResources,
    SessionMemoryResources, SessionPermissionResources, SessionPersistenceResources,
    SessionProjectResources, SessionSandboxResources, SessionTitleResources, SessionTurnResources,
    SessionWorkspaceResources,
};

/// All per-session state shared by both runners. Construction at startup
/// is done once via [`SessionRuntime::build`]; per-turn engines are
/// assembled via [`SessionRuntime::build_engine`].
pub struct SessionRuntime {
    pub(in crate::session::session_runtime) turn_coordinator:
        crate::session_runtime::SessionTurnCoordinator,
    pub(in crate::session::session_runtime) execution: SessionExecutionResources,
    pub(in crate::session::session_runtime) catalog_resources: SessionCatalogResources,
    pub(in crate::session::session_runtime) config_resources: SessionConfigResources,
    pub(in crate::session::session_runtime) project_resources: SessionProjectResources,
    pub(in crate::session::session_runtime) persistence: SessionPersistenceResources,
    pub(in crate::session::session_runtime) title_resources: SessionTitleResources,
    pub(in crate::session::session_runtime) turn_resources: SessionTurnResources,
    pub(in crate::session::session_runtime) command_resources: SessionCommandResources,
    pub(in crate::session::session_runtime) lifecycle_resources: SessionLifecycleResources,
    pub(in crate::session::session_runtime) workspace_resources: SessionWorkspaceResources,
    pub(in crate::session::session_runtime) engine_config_resources: SessionEngineConfigResources,
    pub(in crate::session::session_runtime) engine_state_resources: SessionEngineStateResources,
    pub(in crate::session::session_runtime) integration_resources: SessionIntegrationResources,
    pub(in crate::session::session_runtime) handle_resources: SessionHandleResources,
    pub(in crate::session::session_runtime) permission_resources: SessionPermissionResources,
    pub(in crate::session::session_runtime) agent_catalog_resources: SessionAgentCatalogResources,
    pub(in crate::session::session_runtime) memory_resources: SessionMemoryResources,
    pub(in crate::session::session_runtime) sandbox_resources: SessionSandboxResources,
    pub(in crate::session::session_runtime) history_resources: SessionHistoryResources,
    pub(in crate::session::session_runtime) hook_resources: SessionHookResources,
}
