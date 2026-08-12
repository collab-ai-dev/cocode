//! Tests for the shared install pipeline.
//!
//! The transaction tests use a directory marketplace so they exercise the
//! real resolver, staging, validation, publication, ledger, and settings
//! activation paths without network access.

use super::*;
use crate::dependency::ResolutionResult;
use crate::marketplace::MarketplaceManager;
use crate::schemas::MarketplaceSource;
use crate::schemas::PluginAuthor;
use crate::schemas::PluginMarketplace;
use crate::schemas::PluginSource;

fn marketplace_entry(name: &str, dependencies: Option<Vec<&str>>) -> PluginMarketplaceEntry {
    PluginMarketplaceEntry {
        name: name.to_string(),
        source: PluginSource::RelativePath(format!("./plugins/{name}")),
        version: Some("1.0.0".to_string()),
        description: None,
        author: None,
        category: None,
        tags: None,
        strict: true,
        homepage: None,
        license: None,
        keywords: None,
        dependencies: dependencies.map(|items| {
            items
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect()
        }),
    }
}

fn configure_directory_marketplace(
    plugins_dir: &Path,
    marketplace_dir: &Path,
    entries: Vec<PluginMarketplaceEntry>,
) {
    std::fs::create_dir_all(marketplace_dir).expect("marketplace dir");
    let marketplace = PluginMarketplace {
        name: "test-mkt".to_string(),
        owner: PluginAuthor {
            name: "Test Owner".to_string(),
            email: None,
            url: None,
        },
        plugins: entries,
        renames: None,
        force_remove_deleted_plugins: None,
        metadata: None,
        allow_cross_marketplace_dependencies_on: None,
    };
    std::fs::write(
        marketplace_dir.join("marketplace.json"),
        serde_json::to_vec_pretty(&marketplace).expect("serialize marketplace"),
    )
    .expect("write marketplace");

    MarketplaceManager::new(plugins_dir.to_path_buf())
        .register_marketplace(
            "test-mkt",
            MarketplaceSource::Directory {
                path: marketplace_dir.display().to_string(),
            },
            &marketplace_dir.display().to_string(),
        )
        .expect("register marketplace");
}

fn write_plugin(marketplace_dir: &Path, name: &str) {
    let dir = marketplace_dir.join("plugins").join(name);
    std::fs::create_dir_all(&dir).expect("plugin dir");
    std::fs::write(
        dir.join("PLUGIN.toml"),
        format!("name = \"{name}\"\nversion = \"1.0.0\"\n"),
    )
    .expect("write manifest");
}

#[test]
fn parse_install_target_no_marketplace() {
    assert_eq!(
        parse_install_target("my-plugin"),
        ("my-plugin".to_string(), None)
    );
}

#[test]
fn parse_install_target_with_marketplace() {
    assert_eq!(
        parse_install_target("my-plugin@official"),
        ("my-plugin".to_string(), Some("official".to_string()))
    );
}

#[test]
fn parse_install_target_trims_whitespace() {
    assert_eq!(
        parse_install_target("  my-plugin @ official  "),
        ("my-plugin".to_string(), Some("official".to_string()))
    );
}

#[tokio::test]
async fn install_returns_no_marketplaces_when_unconfigured() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = install_plugin_from_marketplace(
        tmp.path(),
        None,
        &EnterprisePolicy::default(),
        "anything@nowhere",
        PluginScope::User,
    )
    .await;
    match r {
        Err(InstallError::NoMarketplacesConfigured) => (),
        other => panic!("expected NoMarketplacesConfigured, got {other:?}"),
    }
}

#[tokio::test]
async fn install_commits_complete_dependency_closure_with_provenance() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugins_dir = tmp.path().join("config-plugins");
    let settings_dir = tmp.path().join("config");
    let marketplace_dir = tmp.path().join("marketplace");
    configure_directory_marketplace(
        &plugins_dir,
        &marketplace_dir,
        vec![
            marketplace_entry("root", Some(vec!["dep"])),
            marketplace_entry("dep", None),
        ],
    );
    write_plugin(&marketplace_dir, "root");
    write_plugin(&marketplace_dir, "dep");

    let outcome = install_plugin_from_marketplace(
        &plugins_dir,
        Some(&settings_dir),
        &EnterprisePolicy::default(),
        "root@test-mkt",
        PluginScope::User,
    )
    .await
    .expect("install closure");

    assert!(outcome.install_path.join("PLUGIN.toml").is_file());
    assert_eq!(outcome.dep_note, " (+ 1 dependency)");
    let enabled = read_enabled_plugins(Some(&settings_dir));
    assert!(enabled.contains(&PluginId::new("root", "test-mkt")));
    assert!(enabled.contains(&PluginId::new("dep", "test-mkt")));

    let installed = InstalledPluginsManager::load(plugins_dir.join("installed_plugins.json"))
        .expect("load ledger");
    for id in ["root@test-mkt", "dep@test-mkt"] {
        let entry = installed
            .data()
            .plugins
            .get(id)
            .and_then(|entries| entries.first())
            .expect("ledger entry");
        assert!(entry.artifact_sha256.is_some());
        assert_eq!(entry.artifact_file_count, Some(1));
        assert!(entry.source.is_some());
    }
}

#[tokio::test]
async fn install_failure_leaves_no_partial_dependency_closure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugins_dir = tmp.path().join("config-plugins");
    let settings_dir = tmp.path().join("config");
    let marketplace_dir = tmp.path().join("marketplace");
    configure_directory_marketplace(
        &plugins_dir,
        &marketplace_dir,
        vec![
            marketplace_entry("root", Some(vec!["dep"])),
            marketplace_entry("dep", None),
        ],
    );
    write_plugin(&marketplace_dir, "root");
    // The dependency is declared but intentionally absent from the source.

    let result = install_plugin_from_marketplace(
        &plugins_dir,
        Some(&settings_dir),
        &EnterprisePolicy::default(),
        "root@test-mkt",
        PluginScope::User,
    )
    .await;

    assert!(result.is_err());
    assert!(read_enabled_plugins(Some(&settings_dir)).is_empty());
    assert!(!plugins_dir.join("installed_plugins.json").exists());
    let cache_root = plugins_dir.join("cache").join("test-mkt");
    assert!(!cache_root.join("root").exists());
    assert!(!cache_root.join("dep").exists());
}

#[tokio::test]
async fn settings_activation_failure_rolls_back_published_paths_and_ledger() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugins_dir = tmp.path().join("config-plugins");
    let settings_dir = tmp.path().join("config");
    let marketplace_dir = tmp.path().join("marketplace");
    configure_directory_marketplace(
        &plugins_dir,
        &marketplace_dir,
        vec![marketplace_entry("root", None)],
    );
    write_plugin(&marketplace_dir, "root");
    std::fs::create_dir_all(settings_dir.join("settings.json"))
        .expect("make settings path non-regular");

    let result = install_plugin_from_marketplace(
        &plugins_dir,
        Some(&settings_dir),
        &EnterprisePolicy::default(),
        "root@test-mkt",
        PluginScope::User,
    )
    .await;

    assert!(matches!(result, Err(InstallError::SettingsWriteFailed(_))));
    assert!(!plugins_dir.join("installed_plugins.json").exists());
    assert!(
        !plugins_dir.join("cache/test-mkt/root/1.0.0").exists(),
        "published plugin must be removed"
    );
}

#[test]
fn dep_note_uses_canonical_plus_n_suffix() {
    use crate::dependency::format_dependency_count_suffix;
    use crate::identifier::PluginId;
    let dep = |n: usize| -> Vec<PluginId> {
        (0..n)
            .map(|i| PluginId::new(format!("dep{i}"), "mkt".to_string()))
            .collect()
    };
    assert_eq!(format_dependency_count_suffix(&dep(0)), "");
    assert_eq!(format_dependency_count_suffix(&dep(1)), " (+ 1 dependency)");
    assert_eq!(
        format_dependency_count_suffix(&dep(2)),
        " (+ 2 dependencies)"
    );
}

#[test]
fn format_resolution_renders_each_variant() {
    use crate::identifier::PluginId;
    let cycle = ResolutionResult::Cycle {
        chain: vec![
            PluginId::new("a", "m"),
            PluginId::new("b", "m"),
            PluginId::new("a", "m"),
        ],
    };
    assert!(format_resolution(&cycle).contains("Dependency cycle"));

    let cross = ResolutionResult::CrossMarketplace {
        dependency: PluginId::new("dep", "other"),
        required_by: PluginId::new("root", "m"),
    };
    let cross_msg = format_resolution(&cross);
    assert!(cross_msg.contains("cross-marketplace"));
    assert!(cross_msg.contains("dep@other"));

    let not_found = ResolutionResult::NotFound {
        missing: PluginId::new("missing", "m2"),
        required_by: PluginId::new("root", "m"),
    };
    let nf_msg = format_resolution(&not_found);
    assert!(nf_msg.contains("'missing@m2'"));
    assert!(nf_msg.contains("'m2' marketplace"));
}

#[test]
fn write_and_read_enabled_plugins_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let closure = vec![
        crate::identifier::PluginId::new("foo", "official"),
        crate::identifier::PluginId::new("bar", "official"),
    ];
    write_enabled_plugins(tmp.path(), &closure).expect("write");
    let read_back = read_enabled_plugins(Some(tmp.path()));
    assert_eq!(read_back.len(), 2);
    assert!(read_back.contains(&closure[0]));
    assert!(read_back.contains(&closure[1]));
}

#[test]
fn write_enabled_plugins_preserves_existing_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("settings.json"),
        r#"{ "show_thinking": true, "enabled_plugins": { "keep@x": { "enabled": true } } }"#,
    )
    .unwrap();
    let closure = vec![crate::identifier::PluginId::new("new", "official")];
    write_enabled_plugins(tmp.path(), &closure).expect("write");
    let raw = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["show_thinking"], true);
    assert!(v["enabled_plugins"]["keep@x"]["enabled"].as_bool().unwrap());
    assert!(
        v["enabled_plugins"]["new@official"]["enabled"]
            .as_bool()
            .unwrap()
    );
}
