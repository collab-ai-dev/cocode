use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::session_runtime::SessionHandle;

pub async fn build_plugin_dialog_payload(
    session: &SessionHandle,
) -> coco_types::PluginDialogPayload {
    let cfg = session.current_engine_config().await;
    let project_dir = cfg.workspace_cwd();
    let config_home = session.config_home().clone();
    let plugins = coco_plugins::load_all_installed_plugins(&config_home, &project_dir);
    let policy = coco_plugins::security::EnterprisePolicy::from_managed_settings();

    let installed = plugins
        .iter()
        .map(|plugin| {
            let id = plugin.id.to_string();
            let blocked_by_policy = {
                let parsed = coco_plugins::identifier::PluginId::parse(&id);
                matches!(
                    coco_plugins::security::check_policy(&parsed, true, &policy),
                    coco_plugins::security::PolicyVerdict::BlockedPlugin { .. }
                        | coco_plugins::security::PolicyVerdict::BlockedMarketplace { .. }
                        | coco_plugins::security::PolicyVerdict::UnapprovedMarketplace { .. }
                        | coco_plugins::security::PolicyVerdict::UserScopeForbidden
                )
            };
            let source = match &plugin.load_source {
                coco_plugins::loader::PluginLoadSource::Marketplace { marketplace } => {
                    format!("marketplace:{marketplace}")
                }
                coco_plugins::loader::PluginLoadSource::SessionDir => "local".to_string(),
                coco_plugins::loader::PluginLoadSource::Builtin => "builtin".to_string(),
            };
            let options = plugin
                .manifest
                .user_config
                .as_ref()
                .map(|config| {
                    let mut rows = config
                        .iter()
                        .map(|(key, option)| coco_types::PluginDialogOptionRow {
                            key: key.clone(),
                            title: option.title.clone(),
                            description: option.description.clone(),
                            value_type: format!("{:?}", option.config_type).to_ascii_lowercase(),
                            required: option.required.unwrap_or(false),
                            current_value: option.default.clone(),
                        })
                        .collect::<Vec<_>>();
                    rows.sort_by(|a, b| a.key.cmp(&b.key));
                    rows
                })
                .unwrap_or_default();
            let mcp_servers = coco_plugins::mcp_bridge::load_plugin_mcp_servers(plugin)
                .into_iter()
                .map(|server| {
                    let display_name = server
                        .name
                        .strip_prefix("plugin:")
                        .unwrap_or(&server.name)
                        .to_string();
                    coco_types::PluginDialogMcpServerRow {
                        name: server.name,
                        display_name,
                        enabled: true,
                        needs_config: false,
                        tools: Vec::new(),
                        actions: vec![coco_types::PluginDialogAction {
                            label: "Show plugin info".to_string(),
                            plugin_args: format!("info {}", plugin.id.name),
                        }],
                    }
                })
                .collect();
            let mut actions = Vec::new();
            if plugin.enabled {
                actions.push(coco_types::PluginDialogAction {
                    label: "Disable plugin".to_string(),
                    plugin_args: format!("disable {id}"),
                });
            } else {
                actions.push(coco_types::PluginDialogAction {
                    label: "Enable plugin".to_string(),
                    plugin_args: format!("enable {id}"),
                });
            }
            actions.push(coco_types::PluginDialogAction {
                label: "Uninstall plugin".to_string(),
                plugin_args: format!("uninstall {id}"),
            });
            coco_types::PluginDialogInstalledRow {
                id,
                name: plugin.manifest.name.clone(),
                version: plugin.manifest.version.clone(),
                description: plugin.manifest.description.clone(),
                source,
                path: plugin.path.display().to_string(),
                enabled: plugin.enabled,
                blocked_by_policy,
                options,
                mcp_servers,
                actions,
            }
        })
        .collect();

    let skills = build_plugin_dialog_skill_rows(
        &session.skill_manager(),
        &cfg.skill_overrides,
        &config_home,
        coco_model_card::bytes_per_token_for_model(&cfg.model_id),
    );

    let plugins_dir = config_home.join("plugins");
    let mut manager = coco_plugins::marketplace::MarketplaceManager::new(plugins_dir);
    let known = manager.load_known_marketplaces();
    let mut marketplaces = Vec::new();
    for (name, known_marketplace) in known {
        let _ = manager.load_cached_marketplace(&name);
        let plugin_count = manager
            .cached_marketplace(&name)
            .map(|marketplace| i64::try_from(marketplace.plugins.len()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        marketplaces.push(coco_types::PluginDialogMarketplaceRow {
            official: coco_plugins::marketplace::is_official_marketplace_name(&name),
            source: Some(format!("{:?}", known_marketplace.source)),
            name: name.clone(),
            plugin_count,
            actions: vec![coco_types::PluginDialogAction {
                label: "Update marketplace".to_string(),
                plugin_args: format!("marketplace update {name}"),
            }],
        });
    }
    marketplaces.sort_by(|a, b| a.name.cmp(&b.name));

    coco_types::PluginDialogPayload {
        installed,
        skills,
        marketplaces,
        errors: Vec::new(),
    }
}

fn build_plugin_dialog_skill_rows(
    skill_manager: &Arc<coco_skills::SkillManager>,
    tiers: &coco_config::SkillOverrideTiers,
    config_home: &Path,
    bytes_per_token: i64,
) -> Vec<coco_types::PluginDialogSkillRow> {
    let usage = coco_skills::usage::load_all(config_home);
    let now_ms = system_time_ms();
    let bytes_per_token = bytes_per_token.max(1);
    let mut rows = skill_manager
        .all_including_conditional()
        .into_iter()
        .filter(|skill| {
            !matches!(
                skill.source,
                coco_skills::SkillSource::Bundled | coco_skills::SkillSource::Plugin { .. }
            )
        })
        .map(|skill| {
            let lock = coco_skills::resolve_skill_override_lock(&skill, tiers);
            let state = lock
                .as_ref()
                .map(|lock| lock.forced_value)
                .unwrap_or_else(|| coco_skills::effective_skill_state(&skill, tiers));
            let usage = usage.get(&skill.name).map(|stats| {
                let elapsed = now_ms.saturating_sub(stats.last_used_at_ms);
                coco_types::PluginDialogSkillUsage {
                    count: stats.usage_count,
                    days_since_use: elapsed / 86_400_000,
                }
            });
            let token_estimate =
                i64::try_from(coco_skills::estimate_skill_frontmatter_bytes(&skill))
                    .unwrap_or(i64::MAX)
                    / bytes_per_token;
            coco_types::PluginDialogSkillRow {
                id: format!("skill:{}", skill.name),
                name: skill.name.clone(),
                description: skill.description.clone(),
                source: plugin_dialog_skill_source(&skill.source),
                override_state: state,
                lock_source: lock.map(|lock| lock.source),
                token_estimate,
                usage,
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        plugin_dialog_skill_source_sort_key(a.source)
            .cmp(plugin_dialog_skill_source_sort_key(b.source))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

fn plugin_dialog_skill_source(source: &coco_skills::SkillSource) -> coco_types::SkillsDialogSource {
    match source {
        coco_skills::SkillSource::Bundled => coco_types::SkillsDialogSource::BuiltIn,
        coco_skills::SkillSource::Project { .. } => coco_types::SkillsDialogSource::Project,
        coco_skills::SkillSource::User { .. } => coco_types::SkillsDialogSource::User,
        coco_skills::SkillSource::Managed { .. } => coco_types::SkillsDialogSource::Policy,
        coco_skills::SkillSource::Plugin { .. } => coco_types::SkillsDialogSource::Plugin,
        coco_skills::SkillSource::Mcp { .. } => coco_types::SkillsDialogSource::Mcp,
    }
}

fn plugin_dialog_skill_source_sort_key(source: coco_types::SkillsDialogSource) -> &'static str {
    match source {
        coco_types::SkillsDialogSource::BuiltIn => "built-in",
        coco_types::SkillsDialogSource::Project => "project",
        coco_types::SkillsDialogSource::User => "user",
        coco_types::SkillsDialogSource::Policy => "policy",
        coco_types::SkillsDialogSource::Plugin => "plugin",
        coco_types::SkillsDialogSource::Mcp => "mcp",
    }
}

fn system_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
