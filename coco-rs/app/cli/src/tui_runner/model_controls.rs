/// Build the TUI's session-frozen model catalog from the resolved
/// `ModelRegistry`. Each registered ` (provider, model_id)` pair becomes
/// one entry; the same `model_id` shared across providers (e.g.
/// `deepseek-v4` under both `deepseek-openai` and `deepseek-anthropic`)
/// yields one entry per provider. Models not paired with any registered
/// provider are unreachable at runtime and therefore not surfaced.
pub(super) fn build_model_catalog(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_tui::state::ModelCatalogEntry> {
    use coco_tui::state::ModelCatalogEntry;
    let mut entries: Vec<ModelCatalogEntry> = runtime_config
        .model_registry
        .resolved
        .iter()
        .map(|((provider, model_id), resolved)| {
            let info = &resolved.info;
            let supported_efforts: Vec<coco_types::ReasoningEffort> = info
                .supported_thinking_levels
                .as_ref()
                .map(|levels| levels.iter().map(|l| l.effort).collect())
                .unwrap_or_default();
            ModelCatalogEntry {
                provider: provider.clone(),
                provider_display: provider_display_label(provider),
                model_id: model_id.clone(),
                display_name: info
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model_id.clone()),
                context_window: Some(info.context_window.get() as i64),
                supported_efforts,
                default_effort: info.default_thinking_level,
            }
        })
        .collect();
    for endpoint in runtime_config.model_roles.moa_presets.values() {
        if entries.iter().any(|entry| {
            entry.provider == endpoint.display_provider()
                && entry.model_id == endpoint.display_model_id()
        }) {
            continue;
        }
        let context_window = runtime_config
            .model_registry
            .resolve(&endpoint.aggregator.provider, &endpoint.aggregator.model_id)
            .map(|resolved| resolved.info.context_window.get() as i64);
        entries.push(ModelCatalogEntry {
            provider: endpoint.display_provider().to_string(),
            provider_display: "MoA".to_string(),
            model_id: endpoint.display_model_id().to_string(),
            display_name: format!("MoA {}", endpoint.display_model_id()),
            context_window,
            supported_efforts: Vec::new(),
            default_effort: None,
        });
    }

    // Stable sort: provider_display → display_name. Matches the
    // picker's section-by-provider rendering.
    entries.sort_by(|a, b| {
        a.provider_display
            .cmp(&b.provider_display)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    entries
}

/// Convert the static model catalog into the wire payload used by the
/// post-login `/models` refresh (`ModelCatalogRefreshed`).
pub(super) fn model_catalog_infos(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::ModelCatalogInfo> {
    build_model_catalog(runtime_config)
        .into_iter()
        .map(|e| coco_types::ModelCatalogInfo {
            provider: e.provider,
            provider_display: e.provider_display,
            model_id: e.model_id,
            display_name: e.display_name,
            context_window: e.context_window,
            supported_efforts: e.supported_efforts,
            default_effort: e.default_effort,
        })
        .collect()
}

pub(super) fn build_provider_statuses(
    runtime_config: &coco_config::RuntimeConfig,
) -> std::collections::HashMap<String, coco_tui::state::ProviderStatus> {
    use coco_tui::state::ProviderStatus;
    use coco_tui::state::ProviderUnavailableReason;

    let resolver = coco_agent_host::provider_login::shared_resolver();
    runtime_config
        .providers
        .iter()
        .map(|(provider, cfg)| {
            let mut unavailable_reasons = Vec::new();
            if cfg.base_url.trim().is_empty() {
                unavailable_reasons.push(ProviderUnavailableReason::MissingBaseUrl);
            }
            // Branch on auth mode so a logged-in OAuth provider isn't mislabeled
            // "missing API key"(env_key is empty for OAuth instances). Reuses
            // the same credential-presence decision as the client-build gate.
            match cfg.auth {
                coco_config::ProviderAuth::OAuth { .. } => {
                    if !coco_inference::model_factory::provider_credential_present(
                        cfg,
                        Some(&resolver),
                    ) {
                        unavailable_reasons.push(ProviderUnavailableReason::NotLoggedIn {
                            provider: cfg.name.clone(),
                        });
                    }
                }
                coco_config::ProviderAuth::ApiKey => {
                    let has_api_key = cfg
                        .resolve_api_key()
                        .is_some_and(|key| !key.trim().is_empty())
                        || cfg.client_options.auth_token.is_some();
                    if !has_api_key {
                        unavailable_reasons.push(ProviderUnavailableReason::MissingApiKey {
                            env_key: cfg.env_key.clone(),
                        });
                    }
                }
            }
            (
                provider.clone(),
                ProviderStatus {
                    provider_display: provider_display_label(provider),
                    unavailable_reasons,
                },
            )
        })
        .collect()
}

/// Build the `/login` picker rows: every OAuth-capable provider instance with
/// its logged-in state. API-key providers are excluded (they authenticate via
/// env var / `providers.json`, not `/login`). Kept CLI-side — like
/// `build_provider_statuses` — since only the CLI can reach `RuntimeConfig`.
pub(super) fn build_login_entries(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::LoginEntryInfo> {
    let resolver = coco_agent_host::provider_login::shared_resolver();
    let mut entries: Vec<coco_types::LoginEntryInfo> = runtime_config
        .providers
        .iter()
        .filter_map(|(name, cfg)| match cfg.auth {
            coco_config::ProviderAuth::OAuth { .. } => Some(coco_types::LoginEntryInfo {
                provider: name.clone(),
                provider_display: provider_display_label(name),
                auth_label: "OAuth".to_string(),
                logged_in: coco_inference::model_factory::provider_credential_present(
                    cfg,
                    Some(&resolver),
                ),
            }),
            coco_config::ProviderAuth::ApiKey => None,
        })
        .collect();
    entries.sort_by(|a, b| a.provider_display.cmp(&b.provider_display));
    entries
}

/// Build the initial `model_by_role` map from
/// `RuntimeConfig.model_roles`. Each role gets a `ModelBinding` with
/// `effort: None` (the engine's resolver picks the model's default
/// thinking level when no explicit effort is set).
pub(super) fn build_model_by_role(
    runtime_config: &coco_config::RuntimeConfig,
) -> std::collections::HashMap<coco_types::ModelRole, coco_tui::state::ModelBinding> {
    use coco_tui::state::ModelBinding;
    use coco_types::ModelRole;
    const ROLES: [ModelRole; 8] = [
        ModelRole::Main,
        ModelRole::Fast,
        ModelRole::Plan,
        ModelRole::Explore,
        ModelRole::Review,
        ModelRole::HookAgent,
        ModelRole::Memory,
        ModelRole::Subagent,
    ];
    let mut out = std::collections::HashMap::new();
    for role in ROLES {
        if let Some(spec) = runtime_config.model_roles.get(role) {
            let display = runtime_config.model_roles.moa_endpoint(role);
            let provider = display
                .map(|endpoint| endpoint.display_provider().to_string())
                .unwrap_or_else(|| spec.provider.clone());
            let model_id = display
                .map(|endpoint| endpoint.display_model_id().to_string())
                .unwrap_or_else(|| spec.model_id.clone());
            let context_window = runtime_config
                .model_registry
                .resolve(&spec.provider, &spec.model_id)
                .map(|resolved| resolved.info.context_window.get() as i64);
            out.insert(
                role,
                ModelBinding {
                    model_id,
                    provider,
                    context_window,
                    effort: None,
                },
            );
        }
    }
    out
}

/// Provider id → human display label. Falls back to the raw id for
/// providers without an explicit label (e.g. user-named custom
/// providers, or `deepseek-openai` / `deepseek-anthropic` which keep
/// their qualified id so the picker can distinguish them).
pub(super) fn provider_display_label(provider: &str) -> String {
    match provider {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "google" => "Google",
        "deepseek" => "DeepSeek",
        "bytedance" => "ByteDance",
        other => return other.to_string(),
    }
    .to_string()
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
    local_app_server_bridge: &coco_agent_host::sdk_server::AppServerLocalBridge,
) {
    let runtime = session;
    let result = match local_app_server_bridge
        .client()
        .set_model_role(
            local_app_server_bridge.handler(),
            coco_types::SetModelRoleParams {
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
    let role_label = title_case_role(changed.role);
    let effort_suffix = changed
        .effort
        .map(|e| format!(" · thinking: {e}"))
        .unwrap_or_default();
    let display_label = if changed.provider == "moa" {
        format!("{}/{}", changed.provider, changed.model_id)
    } else {
        format!("{}/{}", changed.provider, display_name)
    };
    let output = format!("Set {role_label} → {display_label}{effort_suffix}");
    let messages = coco_messages::build_slash_command_messages(
        "model", /*args*/ "", &output, /*is_sensitive*/ false,
    );
    let mut h = runtime.history().lock().await;
    let event_tx_opt = Some(event_tx.clone());
    for msg in messages {
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }
    let is_remote =
        coco_config::EnvSnapshot::from_current_process().is_truthy(coco_config::EnvKey::CocoRemote);
    if let Some(msg) = build_remote_model_change_reminder(changed.role, &display_name, is_remote) {
        coco_query::history_sync::history_push_and_emit(&mut h, msg, &event_tx_opt).await;
    }
}

/// Title-case a `ModelRole` for display (`main` → `Main`).
pub(super) fn title_case_role(role: coco_types::ModelRole) -> String {
    let mut chars = role.as_str().chars();
    chars.next().map_or_else(String::new, |first| {
        format!("{}{}", first.to_uppercase(), chars.as_str())
    })
}

pub(super) fn build_remote_model_change_reminder(
    role: coco_types::ModelRole,
    display_name: &str,
    is_remote: bool,
) -> Option<coco_messages::Message> {
    if !is_remote || role != coco_types::ModelRole::Main {
        return None;
    }
    Some(coco_messages::wrapping::create_system_reminder_message(
        &format!(
            "The model for this session has been changed to {display_name}. You are now running as {display_name}."
        ),
    ))
}
use coco_types::CoreEvent;
use coco_types::ServerNotification;
