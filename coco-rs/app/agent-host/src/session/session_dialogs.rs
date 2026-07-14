use crate::session_runtime::SessionHandle;

pub struct InitialModelStatusInfo {
    pub model_id: String,
    pub provider: String,
    pub default_effort: Option<coco_types::ReasoningEffort>,
}

pub struct InitialSessionUiFlags {
    pub coordinator_mode_active: bool,
    pub file_history_enabled: bool,
}

pub async fn build_available_commands_payload(
    session: &SessionHandle,
) -> Vec<coco_types::SlashCommandInfo> {
    session.current_command_registry().await.snapshot_for_ui()
}

pub fn provider_display_label(provider: &str) -> String {
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

pub fn build_model_catalog_infos(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::ModelCatalogInfo> {
    let mut entries: Vec<coco_types::ModelCatalogInfo> = runtime_config
        .model_registry
        .resolved
        .iter()
        .map(|((provider, model_id), resolved)| {
            let info = &resolved.info;
            let supported_efforts: Vec<coco_types::ReasoningEffort> = info
                .supported_thinking_levels
                .as_ref()
                .map(|levels| levels.iter().map(|level| level.effort).collect())
                .unwrap_or_default();
            coco_types::ModelCatalogInfo {
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
        entries.push(coco_types::ModelCatalogInfo {
            provider: endpoint.display_provider().to_string(),
            provider_display: "MoA".to_string(),
            model_id: endpoint.display_model_id().to_string(),
            display_name: format!("MoA {}", endpoint.display_model_id()),
            context_window,
            supported_efforts: Vec::new(),
            default_effort: None,
        });
    }
    entries.sort_by(|a, b| {
        a.provider_display
            .cmp(&b.provider_display)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    entries
}

pub fn build_model_catalog_payload(session: &SessionHandle) -> Vec<coco_types::ModelCatalogInfo> {
    build_model_catalog_infos(session.runtime_config())
}

pub fn build_model_role_bindings(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::ModelRoleChangedParams> {
    const ROLES: [coco_types::ModelRole; 8] = [
        coco_types::ModelRole::Main,
        coco_types::ModelRole::Fast,
        coco_types::ModelRole::Plan,
        coco_types::ModelRole::Explore,
        coco_types::ModelRole::Review,
        coco_types::ModelRole::HookAgent,
        coco_types::ModelRole::Memory,
        coco_types::ModelRole::Subagent,
    ];
    let mut bindings = Vec::new();
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
            bindings.push(coco_types::ModelRoleChangedParams {
                role,
                model_id,
                provider,
                context_window,
                effort: None,
            });
        }
    }
    bindings
}

pub fn build_model_role_bindings_payload(
    session: &SessionHandle,
) -> Vec<coco_types::ModelRoleChangedParams> {
    build_model_role_bindings(session.runtime_config())
}

pub fn build_provider_status_infos(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::ProviderStatusInfo> {
    let resolver = crate::provider_login::shared_resolver();
    runtime_config
        .providers
        .iter()
        .map(|(provider, cfg)| {
            let mut unavailable_reasons = Vec::new();
            if cfg.base_url.trim().is_empty() {
                unavailable_reasons.push(coco_types::ProviderUnavailableReason::MissingBaseUrl);
            }
            match cfg.auth {
                coco_config::ProviderAuth::OAuth { .. } => {
                    if !coco_inference::model_factory::provider_credential_present(
                        cfg,
                        Some(&resolver),
                    ) {
                        unavailable_reasons.push(
                            coco_types::ProviderUnavailableReason::NotLoggedIn {
                                provider: cfg.name.clone(),
                            },
                        );
                    }
                }
                coco_config::ProviderAuth::ApiKey => {
                    let has_api_key = cfg
                        .resolve_api_key()
                        .is_some_and(|key| !key.trim().is_empty())
                        || cfg.client_options.auth_token.is_some();
                    if !has_api_key {
                        unavailable_reasons.push(
                            coco_types::ProviderUnavailableReason::MissingApiKey {
                                env_key: cfg.env_key.clone(),
                            },
                        );
                    }
                }
            }
            coco_types::ProviderStatusInfo {
                provider: provider.clone(),
                provider_display: provider_display_label(provider),
                unavailable_reasons,
            }
        })
        .collect()
}

pub fn build_provider_status_payload(
    session: &SessionHandle,
) -> Vec<coco_types::ProviderStatusInfo> {
    build_provider_status_infos(session.runtime_config())
}

pub fn build_login_entries(
    runtime_config: &coco_config::RuntimeConfig,
) -> Vec<coco_types::LoginEntryInfo> {
    let resolver = crate::provider_login::shared_resolver();
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

pub fn build_login_entries_payload(session: &SessionHandle) -> Vec<coco_types::LoginEntryInfo> {
    build_login_entries(session.runtime_config())
}

pub fn available_models_payload(session: &SessionHandle) -> Option<Vec<String>> {
    session
        .runtime_config()
        .settings
        .merged
        .available_models
        .clone()
}

pub fn build_initial_model_status_payload(
    session: &SessionHandle,
    fallback_model_id: &str,
) -> InitialModelStatusInfo {
    let runtime_config = session.runtime_config();
    let Some(spec) = runtime_config.model_roles.get(coco_types::ModelRole::Main) else {
        return InitialModelStatusInfo {
            model_id: fallback_model_id.to_string(),
            provider: String::new(),
            default_effort: None,
        };
    };
    let default_effort = runtime_config
        .model_registry
        .resolve(&spec.provider, &spec.model_id)
        .and_then(|resolved| resolved.info.default_thinking_level);
    InitialModelStatusInfo {
        model_id: fallback_model_id.to_string(),
        provider: spec.provider.clone(),
        default_effort,
    }
}

pub fn build_initial_session_ui_flags_payload(session: &SessionHandle) -> InitialSessionUiFlags {
    InitialSessionUiFlags {
        coordinator_mode_active: coco_subagent::is_coordinator_mode(
            &session.runtime_config().features,
        ),
        file_history_enabled: session.file_history_enabled(),
    }
}

pub async fn build_agents_dialog_payload(
    session: &SessionHandle,
) -> coco_types::AgentsDialogPayload {
    let snapshot = session.agent_catalog_snapshot().await;

    let active_source: std::collections::BTreeMap<String, coco_types::AgentSource> = snapshot
        .active()
        .map(|d| (d.name.clone(), d.source))
        .collect();

    let entries = snapshot
        .all()
        .iter()
        .map(|loaded| {
            let def = &loaded.definition;
            let is_overridden = active_source
                .get(&def.name)
                .map(|winning| *winning != def.source)
                .unwrap_or(false);
            coco_types::AgentsDialogEntry {
                name: def.name.clone(),
                description: def.description.clone().unwrap_or_default(),
                source: def.source,
                color: def.color,
                is_overridden,
                source_path: loaded.path.clone(),
            }
        })
        .collect();
    coco_types::AgentsDialogPayload { entries }
}

pub async fn build_active_agent_definitions_payload(
    session: &SessionHandle,
) -> Vec<coco_types::AgentDefinition> {
    session
        .agent_catalog_snapshot()
        .await
        .active()
        .cloned()
        .collect()
}

pub async fn build_permissions_editor_payload(
    session: &SessionHandle,
) -> coco_types::PermissionsEditorPayload {
    use coco_permissions::permissions_store::PermissionStore;

    let cwd = session.workspace_cwd().await;
    let store = coco_permissions::SettingsPermissionStore::new(cwd.clone());

    let (rules, directories, managed_only) = tokio::task::spawn_blocking(move || {
        let by_behavior = store.load_all_rules();
        let rules: Vec<coco_types::PermissionsEditorRule> = by_behavior
            .allow
            .into_iter()
            .chain(by_behavior.ask)
            .chain(by_behavior.deny)
            .map(|r| coco_types::PermissionsEditorRule {
                behavior: r.behavior,
                source: r.source,
                tool_pattern: r.value.tool_pattern,
                rule_content: r.value.rule_content,
            })
            .collect();
        let directories: Vec<coco_types::PermissionsEditorDir> = store
            .load_additional_directories()
            .into_iter()
            .map(|(source, path)| coco_types::PermissionsEditorDir { path, source })
            .collect();
        let managed_only = !store.show_always_allow_options();
        (rules, directories, managed_only)
    })
    .await
    .unwrap_or_else(|_| (Vec::new(), Vec::new(), false));

    coco_types::PermissionsEditorPayload {
        rules,
        directories,
        cwd: cwd.to_string_lossy().into_owned(),
        managed_only,
    }
}

pub async fn build_workflow_dialog_payload(
    session: &SessionHandle,
) -> coco_types::WorkflowDialogPayload {
    let cfg = session.current_engine_config().await;
    let cwd = if let Some(session_cwd) = cfg.session_cwd.as_ref() {
        Some(session_cwd.read().await.clone())
    } else {
        cfg.original_cwd
            .clone()
            .or_else(|| Some(session.original_cwd().clone()))
    };
    let entries = coco_workflow::list_workflows(cwd)
        .into_iter()
        .map(|entry| coco_types::WorkflowDialogEntry {
            name: entry.name,
            description: entry.description,
            source_path: entry.source_path.display().to_string(),
        })
        .collect();
    coco_types::WorkflowDialogPayload { entries }
}

pub async fn enrich_skills_dialog_payload(
    session: &SessionHandle,
    payload: &mut coco_types::SkillsDialogPayload,
) {
    let cfg = session.current_engine_config().await;
    let skills = session.skill_manager();
    coco_commands::handlers::skills::enrich_payload_with_tiers(
        payload,
        &cfg.skill_overrides,
        &skills,
    );
    payload.bytes_per_token = coco_model_card::bytes_per_token_for_model(&cfg.model_id);
}
