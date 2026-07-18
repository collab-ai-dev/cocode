use std::sync::Arc;

use coco_app_server::AppServer;
use coco_types::{McpToolExposure, SessionId};
use tracing::warn;

use crate::app_session::AppSessionHandle;
use crate::app_session_runtime::{
    AppSessionRuntimeBinding, AppSessionRuntimeProfile, build_app_session_runtime_for_resume,
    build_app_session_runtime_for_start, hydrate_app_session_history,
    install_app_session_integrations,
};

use crate::app_server_host::{AppServerHostState, RuntimeReplacementContext};

pub(crate) async fn configure_connection_mcp_bridge(
    profile: &coco_types::ConnectionProfile,
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<AppSessionHandle>>,
) {
    let Some(server_names) = profile
        .client_mcp_server_names()
        .map(std::borrow::ToOwned::to_owned)
    else {
        return;
    };
    if server_names.is_empty() {
        return;
    }
    if let Err(error) = crate::app_server_host::client_mcp_bridge::register_and_connect(
        session.clone(),
        app_server,
        server_names,
    )
    .await
    {
        warn!(session_id = %session.session_id(), %error, "connection MCP bridge registration failed");
    }
}

pub(crate) async fn build_connection_runtime_for_start(
    replacement: RuntimeReplacementContext,
    _state: Arc<AppServerHostState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    prepared: crate::session_start::PreparedStartSession,
    app_server: Arc<AppServer<AppSessionHandle>>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let binding = runtime_binding_from_replacement(&replacement);
    let profile = runtime_profile_from_connection(
        &connection_profile,
        replacement.requires_structured_output,
        prepared.plan_mode_custom_instructions.clone(),
    );
    let session = build_app_session_runtime_for_start(&binding, &profile, &prepared).await?;
    install_connection_runtime_callbacks(&connection_profile, &session, app_server);
    install_app_session_integrations(&binding, session.clone()).await?;
    hydrate_app_session_history(&session, &prepared.session_id, &prepared.initial_messages).await;
    session
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Startup)
        .await;
    // Configure model/permission/accounting on the still-unpublished runtime so
    // it is fully configured before promotion and surface attachment (CS-1 §0.1
    // item 5). apply_session_start_config merges (Option/true-gated), so this
    // does not clobber the profile-applied structured-output/plan-mode state.
    crate::session_start::apply_prepared_session_start(&prepared, &session).await;
    session.persist_mcp_tool_exposure().await;
    // Structured output requested per session: register the StructuredOutput
    // tool and require a structured final result. An invalid schema fails the
    // start (the load factory returns Result) rather than silently ignoring it.
    if let Some(schema) = &prepared.json_schema {
        let raw = serde_json::to_string(schema)?;
        session
            .install_structured_output_tool_if_requested(Some(&raw))
            .await?;
    }
    Ok(session)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_connection_runtime_for_resume(
    replacement: RuntimeReplacementContext,
    _state: Arc<AppServerHostState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    session_id: SessionId,
    cwd: std::path::PathBuf,
    prior_messages: Vec<coco_messages::Message>,
    plan_mode_instructions: Option<String>,
    persisted_mcp_tool_exposure: Option<McpToolExposure>,
    app_server: Arc<AppServer<AppSessionHandle>>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let binding = runtime_binding_from_replacement(&replacement);
    let profile = runtime_profile_from_connection(
        &connection_profile,
        replacement.requires_structured_output,
        plan_mode_instructions,
    );
    let session =
        build_app_session_runtime_for_resume(&binding, &profile, session_id.clone(), cwd).await?;
    restrict_session_mcp_tool_exposure(&session, persisted_mcp_tool_exposure).await;
    install_connection_runtime_callbacks(&connection_profile, &session, app_server);
    install_app_session_integrations(&binding, session.clone()).await?;
    hydrate_app_session_history(&session, &session_id, &prior_messages).await;
    session
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Resume)
        .await;
    Ok(session)
}

async fn restrict_session_mcp_tool_exposure(
    session: &crate::session_runtime::SessionHandle,
    persisted: Option<McpToolExposure>,
) {
    let current = session.current_engine_config().await.mcp_tool_exposure;
    let effective = effective_mcp_tool_exposure_on_resume(persisted, current);
    session
        .update_engine_config(move |config| config.mcp_tool_exposure = effective)
        .await;
    session.persist_mcp_tool_exposure().await;
}

fn effective_mcp_tool_exposure_on_resume(
    persisted: Option<McpToolExposure>,
    current: McpToolExposure,
) -> McpToolExposure {
    McpToolExposure::restrict(persisted.unwrap_or(McpToolExposure::UseTool), current)
}

pub(crate) async fn build_connection_runtime_for_clear(
    replacement: RuntimeReplacementContext,
    _state: Arc<AppServerHostState>,
    connection_profile: Arc<coco_types::ConnectionProfile>,
    session_id: SessionId,
    snapshot: crate::session_runtime::ClearReplacementSnapshot,
    app_server: Arc<AppServer<AppSessionHandle>>,
) -> anyhow::Result<crate::session_runtime::SessionHandle> {
    let binding = runtime_binding_from_replacement(&replacement);
    // Clear/branch is local-only; plan-mode instructions are re-applied by the
    // surface, so the rebuilt clear destination carries none from the profile.
    let profile = runtime_profile_from_connection(
        &connection_profile,
        replacement.requires_structured_output,
        None,
    );
    let session = binding
        .runtime_factory
        .build_with_session_id_and_cwd(
            session_id,
            binding.cwd.clone(),
            profile.callback_requirements.clone(),
        )
        .await?;
    crate::app_session_runtime::apply_app_session_runtime_profile(&profile, &session).await;
    install_connection_runtime_callbacks(&connection_profile, &session, app_server);
    session.apply_clear_replacement_snapshot(snapshot).await;
    install_app_session_integrations(&binding, session.clone()).await?;
    session
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Clear)
        .await;
    Ok(session)
}

fn runtime_binding_from_replacement(
    replacement: &RuntimeReplacementContext,
) -> AppSessionRuntimeBinding {
    AppSessionRuntimeBinding {
        runtime_factory: replacement.runtime_factory.clone(),
        process_runtime: Arc::clone(&replacement.process_runtime),
        cwd: replacement.cwd.clone(),
        integration_options: replacement.integration_options.clone(),
    }
}

fn runtime_profile_from_connection(
    connection_profile: &coco_types::ConnectionProfile,
    requires_structured_output: bool,
    plan_mode_custom_instructions: Option<String>,
) -> AppSessionRuntimeProfile {
    let supplied_agents = connection_profile
        .initialize()
        .agents
        .as_ref()
        .map(crate::app_server_host::initialize_agents::parse_client_agent_definitions)
        .map(|(accepted, _)| accepted)
        .unwrap_or_default();

    // Plan-mode instructions are per-session execution policy: they arrive on
    // `session/start` / `session/resume` (threaded in by the caller), not on the
    // connection's `initialize`.
    AppSessionRuntimeProfile {
        callback_requirements: connection_profile.callback_requirements(),
        plan_mode_custom_instructions,
        supplied_agents,
        requires_structured_output,
    }
}

fn install_connection_runtime_callbacks(
    connection_profile: &coco_types::ConnectionProfile,
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<AppSessionHandle>>,
) {
    crate::app_server_host::hook_callback_bridge::install_runtime_callback(
        Arc::clone(&app_server),
        session,
    );
    if let Some(hooks) = &connection_profile.initialize().hooks {
        crate::app_server_host::hook_callback_bridge::register_initialize_hooks(session, hooks);
    }
}

pub async fn install_app_server_session_runtime_state(
    state: Arc<AppServerHostState>,
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<AppSessionHandle>>,
) {
    crate::app_server_host::hook_callback_bridge::install_runtime_callback(
        Arc::clone(&app_server),
        &session,
    );
    let session_manager = session.session_manager_handle();
    install_app_server_sandbox_reload_subscription(&session, app_server).await;
    let _ = session;
    state.install_session_manager(session_manager).await;
}

pub(crate) async fn install_app_server_sandbox_reload_subscription(
    session: &crate::session_runtime::SessionHandle,
    app_server: Arc<AppServer<AppSessionHandle>>,
) {
    let approval_bridge: coco_sandbox::SandboxApprovalBridgeRef = Arc::new(
        crate::app_server_host::sandbox_approval_bridge::AppServerSandboxApprovalBridge::new(
            app_server,
            session.clone(),
        ),
    );
    session.set_sandbox_approval_bridge(approval_bridge);
    session.install_sandbox_reload_supervisor().await;
}

pub(crate) fn touch_runtime_backed_resumed_session_activity(
    state: &AppServerHostState,
    session_id: SessionId,
) {
    state.touch_session_activity(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_mcp_exposure_uses_use_tool_floor_and_never_widens() {
        use McpToolExposure::{Defer, Load, UseTool};

        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Option::None, Defer),
            UseTool
        );
        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Some(Load), Defer),
            Defer
        );
        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Some(UseTool), Defer),
            UseTool
        );
        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Some(Defer), UseTool),
            UseTool
        );
        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Some(Defer), Defer),
            Defer
        );
        assert_eq!(
            effective_mcp_tool_exposure_on_resume(Some(Defer), Load),
            Defer
        );
    }
}
