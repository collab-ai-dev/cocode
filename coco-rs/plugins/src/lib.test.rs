use super::*;
use std::collections::HashMap;
use std::collections::HashSet;

#[test]
fn test_get_plugin_dirs_includes_both_config_and_project() {
    // The host calls `get_plugin_dirs(config_dir, project_dir)` at
    // startup — the result is the loader's input. Verifies both
    // user-level (`<config_dir>/plugins/*/`) and project-level
    // (`<project_dir>/project config dir/plugins/*/`) directories are surfaced.
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path().join("config");
    let project_dir = tmp.path().join("project");
    let user_plugin = config_dir.join("plugins").join("user-plug");
    let proj_plugin = project_dir
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("plugins")
        .join("proj-plug");
    std::fs::create_dir_all(&user_plugin).expect("mkdir user");
    std::fs::create_dir_all(&proj_plugin).expect("mkdir proj");

    let dirs = get_plugin_dirs(&config_dir, &project_dir);
    assert!(
        dirs.iter().any(|p| p == &user_plugin),
        "user plugin dir missing — got {dirs:?}",
    );
    assert!(
        dirs.iter().any(|p| p == &proj_plugin),
        "project plugin dir missing — got {dirs:?}",
    );
}

#[test]
fn test_get_plugin_dirs_handles_missing_dirs() {
    // No plugin dirs on disk — function must not error, just return empty.
    let tmp = tempfile::tempdir().expect("tempdir");
    let dirs = get_plugin_dirs(
        &tmp.path().join("nope-config"),
        &tmp.path().join("nope-project"),
    );
    assert!(
        dirs.is_empty(),
        "expected empty list when neither plugin dir exists, got {dirs:?}",
    );
}

/// Write a minimal inline plugin dir (`<config>/plugins/<name>/PLUGIN.toml`).
fn write_inline_plugin(config: &std::path::Path, name: &str) {
    let plug = config.join("plugins").join(name);
    std::fs::create_dir_all(&plug).expect("mkdir plugin");
    std::fs::write(
        plug.join("PLUGIN.toml"),
        format!("name = \"{name}\"\nversion = \"1.0.0\"\ndescription = \"{name}\"\n"),
    )
    .expect("write manifest");
}

fn marketplace_entry(name: &str) -> schemas::PluginMarketplaceEntry {
    schemas::PluginMarketplaceEntry {
        name: name.to_string(),
        source: schemas::PluginSource::RelativePath(format!("./plugins/{name}")),
        version: Some("1.0.0".to_string()),
        description: None,
        author: None,
        category: None,
        tags: None,
        strict: true,
        homepage: None,
        license: None,
        keywords: None,
        dependencies: None,
    }
}

fn marketplace_with_renames(
    name: &str,
    plugins: Vec<schemas::PluginMarketplaceEntry>,
    renames: HashMap<String, Option<String>>,
) -> schemas::PluginMarketplace {
    schemas::PluginMarketplace {
        name: name.to_string(),
        owner: schemas::PluginAuthor {
            name: "owner".to_string(),
            email: None,
            url: None,
        },
        plugins,
        renames: Some(renames),
        force_remove_deleted_plugins: None,
        metadata: None,
        allow_cross_marketplace_dependencies_on: None,
    }
}

#[test]
fn test_load_enabled_plugins_inline_dir_enabled_by_default() {
    // An inline (local) plugin under `<config>/plugins/<name>` with a
    // PLUGIN.toml is loaded and enabled by default (no settings entry).
    // Identity is `<name>@inline`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("config");
    let project = tmp.path().join("project");
    write_inline_plugin(&config, "alpha");

    let plugins = load_enabled_plugins(&config, &project);
    assert!(
        plugins
            .iter()
            .any(|p| p.id.name == "alpha" && p.id.marketplace == "inline"),
        "inline plugin 'alpha@inline' should load enabled — got {:?}",
        plugins.iter().map(|p| p.id.to_string()).collect::<Vec<_>>(),
    );
}

#[test]
fn test_load_enabled_plugins_respects_disabled_setting() {
    // settings.json `enabled_plugins["beta@inline"] = false` filters the
    // inline plugin out of the active set — proving the persisted key the
    // `/plugin disable` handler writes (`name@inline`) matches what the
    // loader reads.
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("config");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&config).expect("mkdir config");
    write_inline_plugin(&config, "beta");
    std::fs::write(
        config.join("settings.json"),
        r#"{ "enabled_plugins": { "beta@inline": false } }"#,
    )
    .expect("write settings");

    let plugins = load_enabled_plugins(&config, &project);
    assert!(
        !plugins.iter().any(|p| p.id.name == "beta"),
        "disabled inline plugin 'beta@inline' must be filtered — got {:?}",
        plugins.iter().map(|p| p.id.to_string()).collect::<Vec<_>>(),
    );
}

#[test]
fn resolve_plugin_rename_classifies_success_removed_and_failures() {
    let present: HashSet<String> = ["new"].into_iter().map(String::from).collect();
    let renames: HashMap<String, Option<String>> = [
        ("old".to_string(), Some("new".to_string())),
        ("gone".to_string(), None),
        ("a".to_string(), Some("b".to_string())),
        ("b".to_string(), Some("a".to_string())),
        ("missing".to_string(), Some("target".to_string())),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        resolve_plugin_rename("old", &renames, &present),
        Some(PluginRenameResolution::Renamed {
            to: "new".to_string(),
            chain_depth: 1
        })
    );
    assert_eq!(
        resolve_plugin_rename("gone", &renames, &present),
        Some(PluginRenameResolution::Removed { chain_depth: 1 })
    );
    assert_eq!(
        resolve_plugin_rename("a", &renames, &present),
        Some(PluginRenameResolution::Unresolved {
            reason: RenameUnresolvedReason::Cycle
        })
    );
    assert_eq!(
        resolve_plugin_rename("missing", &renames, &present),
        Some(PluginRenameResolution::Unresolved {
            reason: RenameUnresolvedReason::TargetMissing
        })
    );

    let mut deep = HashMap::new();
    for i in 0..=MAX_PLUGIN_RENAME_CHAIN {
        deep.insert(format!("p{i}"), Some(format!("p{}", i + 1)));
    }
    assert_eq!(
        resolve_plugin_rename("p0", &deep, &present),
        Some(PluginRenameResolution::Unresolved {
            reason: RenameUnresolvedReason::ChainTooDeep
        })
    );
    assert_eq!(resolve_plugin_rename("absent", &renames, &present), None);
}

#[test]
fn plugin_rename_telemetry_fields_match_resolution_shape() {
    assert_eq!(
        plugin_rename_telemetry_fields(&PluginRenameResolution::Renamed {
            to: "new".to_string(),
            chain_depth: 2,
        }),
        PluginRenameTelemetryFields {
            outcome: "renamed",
            chain_depth: Some(2),
            reason: None,
        }
    );
    assert_eq!(
        plugin_rename_telemetry_fields(&PluginRenameResolution::Removed { chain_depth: 1 }),
        PluginRenameTelemetryFields {
            outcome: "removed",
            chain_depth: Some(1),
            reason: None,
        }
    );
    assert_eq!(
        plugin_rename_telemetry_fields(&PluginRenameResolution::Unresolved {
            reason: RenameUnresolvedReason::Cycle,
        }),
        PluginRenameTelemetryFields {
            outcome: "unresolved",
            chain_depth: None,
            reason: Some("cycle"),
        }
    );
}

#[test]
fn load_enabled_plugins_follows_marketplace_rename_and_migrates_settings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("config");
    let project = tmp.path().join("project");
    let plugins_dir = config.join("plugins");
    std::fs::create_dir_all(&plugins_dir).expect("mkdir plugins");

    let mut renames = HashMap::new();
    renames.insert("old".to_string(), Some("new".to_string()));
    let marketplace = marketplace_with_renames("mkt", vec![marketplace_entry("new")], renames);
    let marketplace_path = plugins_dir.join("marketplaces").join("mkt.json");
    std::fs::create_dir_all(marketplace_path.parent().expect("parent")).expect("mkdir mkt");
    std::fs::write(
        &marketplace_path,
        serde_json::to_string_pretty(&marketplace).expect("serialize marketplace"),
    )
    .expect("write marketplace");

    let mut mgr = marketplace::MarketplaceManager::new(plugins_dir.clone());
    mgr.register_marketplace(
        "mkt",
        schemas::MarketplaceSource::File {
            path: marketplace_path.display().to_string(),
        },
        &marketplace_path.display().to_string(),
    )
    .expect("register marketplace");

    let cached_plugin = plugins_dir
        .join("cache")
        .join("mkt")
        .join("new")
        .join("1.0.0");
    std::fs::create_dir_all(&cached_plugin).expect("mkdir cache");
    std::fs::write(
        cached_plugin.join("PLUGIN.toml"),
        "name = \"new\"\nversion = \"1.0.0\"\ndescription = \"new\"\n",
    )
    .expect("write plugin");
    std::fs::write(
        config.join("settings.json"),
        r#"{
            "enabled_plugins": { "old@mkt": { "enabled": true } },
            "plugin_configs": { "old@mkt": { "setting": 1 } },
            "pluginConfigs": {
                "old@mkt": { "setting": 2 },
                "untouched@mkt": { "setting": 3 }
            },
            "other": 1
        }"#,
    )
    .expect("write settings");

    let plugins = load_enabled_plugins(&config, &project);
    assert!(
        plugins
            .iter()
            .any(|plugin| plugin.id.as_str() == "new@mkt" && plugin.enabled),
        "renamed marketplace plugin should load enabled: {:?}",
        plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>()
    );

    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config.join("settings.json")).unwrap())
            .unwrap();
    let enabled = settings["enabled_plugins"]
        .as_object()
        .expect("enabled map");
    assert!(!enabled.contains_key("old@mkt"));
    assert_eq!(enabled["new@mkt"]["enabled"].as_bool(), Some(true));
    let configs = settings["plugin_configs"]
        .as_object()
        .expect("plugin config map");
    assert!(!configs.contains_key("old@mkt"));
    assert_eq!(configs["new@mkt"]["setting"].as_i64(), Some(1));
    assert!(
        !settings["pluginConfigs"]
            .as_object()
            .expect("camel plugin config map")
            .contains_key("old@mkt")
    );
    assert_eq!(
        settings["pluginConfigs"]["new@mkt"]["setting"].as_i64(),
        Some(2)
    );
    assert_eq!(
        settings["pluginConfigs"]["untouched@mkt"]["setting"].as_i64(),
        Some(3)
    );
    assert_eq!(settings["other"].as_i64(), Some(1));
}

#[test]
fn marketplace_rename_respects_disabled_target_over_old_enabled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = tmp.path().join("config");
    let project = tmp.path().join("project");
    let plugins_dir = config.join("plugins");
    std::fs::create_dir_all(&plugins_dir).expect("mkdir plugins");

    let mut renames = HashMap::new();
    renames.insert("old".to_string(), Some("new".to_string()));
    let marketplace = marketplace_with_renames("mkt", vec![marketplace_entry("new")], renames);
    let marketplace_path = plugins_dir.join("marketplaces").join("mkt.json");
    std::fs::create_dir_all(marketplace_path.parent().expect("parent")).expect("mkdir mkt");
    std::fs::write(
        &marketplace_path,
        serde_json::to_string_pretty(&marketplace).expect("serialize marketplace"),
    )
    .expect("write marketplace");

    let mut mgr = marketplace::MarketplaceManager::new(plugins_dir.clone());
    mgr.register_marketplace(
        "mkt",
        schemas::MarketplaceSource::File {
            path: marketplace_path.display().to_string(),
        },
        &marketplace_path.display().to_string(),
    )
    .expect("register marketplace");

    let cached_plugin = plugins_dir
        .join("cache")
        .join("mkt")
        .join("new")
        .join("1.0.0");
    std::fs::create_dir_all(&cached_plugin).expect("mkdir cache");
    std::fs::write(
        cached_plugin.join("PLUGIN.toml"),
        "name = \"new\"\nversion = \"1.0.0\"\ndescription = \"new\"\n",
    )
    .expect("write plugin");
    std::fs::write(
        config.join("settings.json"),
        r#"{
            "enabled_plugins": {
                "old@mkt": { "enabled": true },
                "new@mkt": { "enabled": false }
            }
        }"#,
    )
    .expect("write settings");

    let plugins = load_enabled_plugins(&config, &project);
    assert!(
        plugins.iter().all(|plugin| plugin.id.as_str() != "new@mkt"),
        "disabled rename target must not load enabled: {:?}",
        plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>()
    );

    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config.join("settings.json")).unwrap())
            .unwrap();
    let enabled = settings["enabled_plugins"]
        .as_object()
        .expect("enabled map");
    assert!(!enabled.contains_key("old@mkt"));
    assert_eq!(enabled["new@mkt"]["enabled"].as_bool(), Some(false));
}
