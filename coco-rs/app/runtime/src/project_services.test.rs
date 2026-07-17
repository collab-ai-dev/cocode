//! Project service cache and discovery tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;

use super::*;

/// An activation policy that pre-approves project-scope servers, for tests
/// exercising project-root scoping rather than the approval gate itself
/// (which is covered in `coco-mcp::activation`).
fn pre_approved_policy(project_root: &std::path::Path) -> coco_mcp::McpActivationPolicy {
    coco_mcp::McpActivationPolicy::resolve_with_global(
        &coco_config::global_config::GlobalConfig::default(),
        project_root,
        &coco_config::McpPolicyConfig {
            project_servers_pre_approved: true,
            ..Default::default()
        },
    )
}

#[test]
fn registry_reuses_services_for_same_project_root() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = ProjectRegistry::new();

    let first = registry.get_or_load(&config_home, project_root.clone());
    let second = registry.get_or_load(&config_home, project_root.clone());

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(registry.len(), 1);
    assert_eq!(first.project_root(), project_root.as_path());
}

#[test]
fn registry_separates_project_roots() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_a = temp.path().join("repo-a");
    let project_b = temp.path().join("repo-b");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();
    let registry = ProjectRegistry::new();

    let first = registry.get_or_load(&config_home, project_a);
    let second = registry.get_or_load(&config_home, project_b);

    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(registry.len(), 2);
}

#[test]
fn different_project_roots_resolve_independent_mcp_servers() {
    // Two sessions in different projects must see only their own project's MCP
    // servers — project config is per-project, never cross-contaminated
    // (multi-session project-scope invariant).
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_a = temp.path().join("repo-a");
    let project_b = temp.path().join("repo-b");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();
    std::fs::write(
        project_a.join(".mcp.json"),
        r#"{"mcpServers":{"only-in-a":{"command":"server-a","args":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        project_b.join(".mcp.json"),
        r#"{"mcpServers":{"only-in-b":{"command":"server-b","args":[]}}}"#,
    )
    .unwrap();
    let registry = ProjectRegistry::new();

    let services_a = registry.get_or_load(&config_home, project_a.clone());
    let services_b = registry.get_or_load(&config_home, project_b.clone());

    let names_a: Vec<String> = services_a
        .mcp_servers(&config_home, &project_a, &pre_approved_policy(&project_a))
        .into_iter()
        .map(|server| server.name)
        .collect();
    let names_b: Vec<String> = services_b
        .mcp_servers(&config_home, &project_b, &pre_approved_policy(&project_b))
        .into_iter()
        .map(|server| server.name)
        .collect();

    assert_eq!(names_a, vec!["only-in-a".to_string()]);
    assert_eq!(names_b, vec!["only-in-b".to_string()]);
}

#[test]
fn registry_reload_replaces_cached_entry() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = ProjectRegistry::new();

    let first = registry.get_or_load(&config_home, project_root.clone());
    let second = registry.reload(&config_home, project_root.clone());
    let third = registry.get_or_load(&config_home, project_root);

    assert!(!Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&second, &third));
    assert_eq!(registry.len(), 1);
}

#[test]
fn project_services_tracks_project_settings_path() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();

    let services = ProjectServices::load(&config_home, project_root.clone());

    assert_eq!(
        services.project_config_snapshot().settings_path(),
        coco_config::global_config::project_settings_path(&project_root).as_path()
    );
}

#[test]
fn registry_refreshes_entry_when_project_settings_change() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    let settings_dir = project_root.join(coco_utils_common::COCO_CONFIG_DIR_NAME);
    let settings_path = settings_dir.join("settings.json");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&settings_dir).unwrap();
    let registry = ProjectRegistry::new();

    let first = registry.get_or_load(&config_home, project_root.clone());
    assert!(!first.project_config_snapshot().has_changed());

    std::fs::write(&settings_path, "{}").unwrap();
    assert!(first.project_config_snapshot().has_changed());

    // A stale project settings file rebuilds the entry: the next lookup returns
    // a fresh `ProjectServices` (new `Arc`, freshly-loaded plugin catalog) so a
    // later session sees the current settings, while `first`'s snapshot — held
    // by an already-running session — is left untouched.
    let second = registry.get_or_load(&config_home, project_root.clone());
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(!second.project_config_snapshot().has_changed());

    // With no further change, subsequent lookups reuse the rebuilt entry.
    let third = registry.get_or_load(&config_home, project_root);
    assert!(Arc::ptr_eq(&second, &third));
}

#[test]
fn registry_idle_eviction_keeps_attached_services() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = ProjectRegistry::new();

    let _attached = registry.get_or_load(&config_home, project_root);

    assert_eq!(registry.evict_idle(Duration::ZERO), 0);
    assert_eq!(registry.evict_idle(Duration::ZERO), 0);
    assert_eq!(registry.len(), 1);
}

#[test]
fn registry_idle_eviction_removes_unattached_services_after_grace() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = ProjectRegistry::new();

    {
        let _attached = registry.get_or_load(&config_home, project_root);
    }

    assert_eq!(registry.evict_idle(Duration::ZERO), 0);
    assert_eq!(registry.evict_idle(Duration::ZERO), 1);
    assert_eq!(registry.len(), 0);
}

#[tokio::test]
async fn registry_background_idle_eviction_removes_unattached_services() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let registry = Box::leak(Box::new(ProjectRegistry::new()));

    {
        let _attached = registry.get_or_load(&config_home, project_root);
    }

    let manager =
        ProjectRegistryManager::start(registry, Duration::ZERO, Duration::from_millis(10));
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(manager);

    assert_eq!(registry.len(), 0);
}

#[test]
fn mcp_servers_use_project_root_and_session_cwd() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    let session_cwd = project_root.join("nested");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(project_root.join(".coco")).unwrap();
    let local_dir = session_cwd.join(format!("{}.local", coco_utils_common::COCO_CONFIG_DIR_NAME));
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(
        project_root.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "project": {"command": "project-cmd", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        local_dir.join("mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "local": {"command": "local-cmd", "args": []}
            }
        })
        .to_string(),
    )
    .unwrap();
    let services = ProjectServices::load(&config_home, project_root.clone());
    assert_eq!(services.project_root(), project_root.as_path());

    let servers = services.mcp_servers(
        &config_home,
        &session_cwd,
        &pre_approved_policy(&project_root),
    );
    let by_name: HashMap<_, _> = servers
        .into_iter()
        .map(|server| (server.name.clone(), server))
        .collect();

    assert_eq!(by_name["project"].scope, coco_mcp::ConfigScope::Project);
    assert_eq!(by_name["local"].scope, coco_mcp::ConfigScope::Local);
}

#[test]
fn lsp_servers_are_empty_without_plugin_contributions() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();

    let services = ProjectServices::load(&config_home, project_root);

    assert!(services.lsp_servers().servers.is_empty());
}

#[test]
fn agent_search_paths_include_enabled_plugin_agent_dirs() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    let plugin_dir = config_home.join("plugins").join("agent-pack");
    let plugin_agents_dir = plugin_dir.join("agents");
    std::fs::create_dir_all(&plugin_agents_dir).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::write(
        plugin_dir.join("PLUGIN.toml"),
        r#"name = "agent-pack"
version = "1.0.0"
description = "agent pack"
"#,
    )
    .unwrap();
    std::fs::write(plugin_agents_dir.join("reviewer.md"), "# reviewer").unwrap();
    let services = ProjectServices::load(&config_home, project_root.clone());

    let paths = services.agent_search_paths(&config_home, &project_root);

    assert!(
        paths
            .plugin_dirs
            .iter()
            .any(|dir| dir.plugin_name == "agent-pack" && dir.dir == plugin_agents_dir),
        "plugin agent dir missing from search paths: {:?}",
        paths.plugin_dirs
    );
}

#[test]
fn build_skill_manager_matches_base_catalog_without_plugins() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let gates = coco_skills::SkillLoadGates::default();
    let services = ProjectServices::load(&config_home, project_root.clone());

    let base = coco_skills::build_session_skill_manager(&config_home, &project_root, &gates);
    let via_project_services = services.build_skill_manager(&config_home, &project_root, &gates);

    assert_eq!(via_project_services.len(), base.len());
}

#[test]
fn register_plugin_hooks_is_noop_without_plugins() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    let services = ProjectServices::load(&config_home, project_root);
    let registry = coco_hooks::HookRegistry::new();

    let count = services.register_plugin_hooks(&registry);

    assert_eq!(count, 0);
    assert_eq!(registry.len(), 0);
}

#[test]
fn build_command_registry_matches_base_registry_without_plugins() {
    let temp = tempdir().unwrap();
    let config_home = temp.path().join("home");
    let project_root = temp.path().join("repo");
    let home = temp.path().join("user-home");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let services = ProjectServices::load(&config_home, project_root.clone());
    let skill_manager = coco_skills::SkillManager::new();
    let features = coco_types::Features::with_defaults();
    let loop_config = coco_config::LoopConfig::default();
    let skill_overrides = coco_config::SkillOverrideTiers::default();

    let base = coco_commands::build_command_registry(
        &skill_manager,
        &[],
        coco_types::UserType::from_env(),
        features.clone(),
        loop_config.clone(),
        coco_commands::handlers::mcp::McpCommandContext::for_cwd(project_root.clone()),
        project_root.clone(),
        home.clone(),
        None,
        &skill_overrides,
    );
    let via_project_services = services.build_command_registry(
        &skill_manager,
        coco_types::UserType::from_env(),
        features,
        loop_config,
        coco_config::McpPolicyConfig::default(),
        project_root,
        home,
        None,
        &skill_overrides,
    );

    assert_eq!(via_project_services.len(), base.len());
}
