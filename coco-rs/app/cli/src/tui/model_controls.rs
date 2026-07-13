pub(super) fn model_catalog_from_infos(
    infos: Vec<coco_types::ModelCatalogInfo>,
) -> Vec<coco_tui::state::ModelCatalogEntry> {
    use coco_tui::state::ModelCatalogEntry;
    infos
        .into_iter()
        .map(|info| ModelCatalogEntry {
            provider: info.provider,
            provider_display: info.provider_display,
            model_id: info.model_id,
            display_name: info.display_name,
            context_window: info.context_window,
            supported_efforts: info.supported_efforts,
            default_effort: info.default_effort,
        })
        .collect()
}

pub(super) fn provider_statuses_from_infos(
    infos: Vec<coco_types::ProviderStatusInfo>,
) -> std::collections::HashMap<String, coco_tui::state::ProviderStatus> {
    use coco_tui::state::ProviderStatus;

    infos
        .into_iter()
        .map(|status| {
            (
                status.provider,
                ProviderStatus {
                    provider_display: status.provider_display,
                    unavailable_reasons: status.unavailable_reasons,
                },
            )
        })
        .collect()
}

/// Convert the host's initial model-role payload into TUI state.
pub(super) fn build_model_by_role_from_payload(
    payload: Vec<coco_types::ModelRoleChangedParams>,
) -> std::collections::HashMap<coco_types::ModelRole, coco_tui::state::ModelBinding> {
    use coco_tui::state::ModelBinding;
    payload
        .into_iter()
        .map(|binding| {
            (
                binding.role,
                ModelBinding {
                    model_id: binding.model_id,
                    provider: binding.provider,
                    context_window: binding.context_window,
                    effort: binding.effort,
                },
            )
        })
        .collect()
}

/// Apply a ` (role, provider, model_id, effort)` selection through the local
/// AppServer handler, which updates the live runtime in memory and emits
/// [`ServerNotification::ModelRoleChanged`] so the TUI refreshes its
/// `model_by_role` mirror (and, when `role == Main`, the status-bar
/// fields).
/// **No file write.** Users who want the binding to survive across
/// sessions edit `the global config file::model_roles.<role>.primary` themselves.
/// The picker is for fast experimentation, not persistence.
/// Non-Main roles take effect on the next turn that drives that role.
/// Main effort takes effect immediately; Main model_id changes only
/// take effect on next session restart — see
/// [`SessionRuntime::client_for_role`] doc-comment.
pub(super) async fn apply_role_through_app_server(
    session: &crate::session_runtime::SessionHandle,
    role: coco_types::ModelRole,
    provider: String,
    model_id: String,
    effort: Option<coco_types::ReasoningEffort>,
    event_tx: &tokio::sync::mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &coco_agent_host::app_server_host::AppServerLocalBridge,
) {
    let runtime = session;
    let result = match local_app_server_bridge
        .client()
        .set_model_role(
            local_app_server_bridge.handler(),
            coco_types::SetModelRoleParams {
                target: interactive_target(local_app_server_bridge),
                role,
                provider: provider.clone(),
                model_id: model_id.clone(),
                effort,
            },
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(
                role = %role.as_str(),
                provider = %provider,
                model_id = %model_id,
                error = %error,
                "control/setModelRole failed; reverting picker mirror"
            );
            let _ = event_tx
                .send(CoreEvent::Protocol(ServerNotification::Error(
                    coco_types::ErrorParams {
                        message: format!(
                            "failed to apply {role_label} -> {provider}/{model_id}: {error}",
                            role_label = role.as_str(),
                        ),
                        category: Some("model_role_apply_failed".to_string()),
                        retryable: true,
                    },
                )))
                .await;
            return;
        }
    };
    let coco_types::SetModelRoleResult {
        changed,
        display_name,
    } = result;
    tracing::info!(
        role = %changed.role.as_str(),
        provider = %changed.provider,
        model_id = %changed.model_id,
        effort = ?changed.effort,
        "applied in-memory model-role override through local AppServer (not persisted)"
    );

    // Tool-style confirmation for the `/model` picker (no-args → modal →
    // Enter). Rendered `❯ /model` + `⎿ Set …` like every slash result, but
    // `System` (transcript-only): model/role selection is a tool-config
    // action — the LLM must NOT see it in its context. Engine-side push so
    // it fires ONLY for the picker; the Ctrl+T effort cycle reuses
    // `ModelRoleChanged` but stays silent (status-bar only).
    let is_remote =
        coco_config::EnvSnapshot::from_current_process().is_truthy(coco_config::EnvKey::CocoRemote);
    coco_agent_host::session_messages::append_model_role_change_to_history_and_emit(
        runtime,
        event_tx.clone(),
        &changed,
        &display_name,
        is_remote,
    )
    .await;
}
use super::interactive_target;
use coco_types::{CoreEvent, ServerNotification};
