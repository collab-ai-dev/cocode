use std::collections::HashSet;

use pretty_assertions::assert_eq;

use super::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_toml(dir: &std::path::Path, content: &str) {
    std::fs::write(dir.join("PLUGIN.toml"), content).expect("write PLUGIN.toml");
}

fn write_json(dir: &std::path::Path, content: &str) {
    std::fs::write(dir.join("plugin.json"), content).expect("write plugin.json");
}

// ---------------------------------------------------------------------------
// load_from_dir
// ---------------------------------------------------------------------------

#[test]
fn test_load_from_dir_toml() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("my-plugin");
    std::fs::create_dir_all(&dir).expect("mkdir");
    write_toml(
        &dir,
        r#"
name = "my-plugin"
version = "1.0.0"
description = "Hello"
"#,
    );

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let plugin = loader
        .load_from_dir(&dir, PluginLoadSource::SessionDir, None)
        .expect("should load");

    assert_eq!(plugin.id.name, "my-plugin");
    assert_eq!(plugin.id.marketplace, "inline");
    assert_eq!(plugin.manifest.version.as_deref(), Some("1.0.0"));
    assert!(plugin.enabled);
}

#[test]
fn test_load_from_dir_json_fallback() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("json-plugin");
    std::fs::create_dir_all(&dir).expect("mkdir");
    write_json(
        &dir,
        r#"{"name":"json-plugin","version":"2.0.0","description":"from json"}"#,
    );

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let plugin = loader
        .load_from_dir(&dir, PluginLoadSource::SessionDir, None)
        .expect("should load");

    assert_eq!(plugin.id.name, "json-plugin");
    assert_eq!(plugin.manifest.version.as_deref(), Some("2.0.0"));
}

#[test]
fn test_load_from_dir_no_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("empty");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let err = loader
        .load_from_dir(&dir, PluginLoadSource::SessionDir, None)
        .expect_err("should fail");

    assert!(err.message.contains("no PLUGIN.toml or plugin.json found"));
}

#[test]
fn test_load_from_dir_invalid_name_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("bad");
    std::fs::create_dir_all(&dir).expect("mkdir");
    write_toml(&dir, r#"name = "bad name with spaces""#);

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let err = loader
        .load_from_dir(&dir, PluginLoadSource::SessionDir, None)
        .expect_err("should reject invalid name");

    assert!(err.message.contains("spaces"));
}

#[test]
fn test_load_from_dir_marketplace_name_override() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("mkt-plugin");
    std::fs::create_dir_all(&dir).expect("mkdir");
    write_toml(&dir, r#"name = "mkt-plugin""#);

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let plugin = loader
        .load_from_dir(
            &dir,
            PluginLoadSource::Marketplace {
                marketplace: "my-mkt".to_string(),
            },
            Some("my-mkt"),
        )
        .expect("should load");

    assert_eq!(plugin.id.marketplace, "my-mkt");
}

// ---------------------------------------------------------------------------
// load_session_plugins
// ---------------------------------------------------------------------------

#[test]
fn test_load_session_plugins_multiple() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let a = tmp.path().join("plugin-a");
    let b = tmp.path().join("plugin-b");
    std::fs::create_dir_all(&a).expect("mkdir");
    std::fs::create_dir_all(&b).expect("mkdir");
    write_toml(&a, r#"name = "alpha""#);
    write_toml(&b, r#"name = "beta""#);

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let result = loader.load_session_plugins(&[a, b]);

    assert_eq!(result.plugins.len(), 2);
    assert!(result.errors.is_empty());

    let names: Vec<&str> = result.plugins.iter().map(|p| p.id.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

#[test]
fn test_load_session_plugins_nonexistent_dir() {
    let loader = PluginLoader::new("/tmp".into());
    let result = loader.load_session_plugins(&["/nonexistent/path".into()]);

    assert!(result.plugins.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].message.contains("does not exist"));
}

// ---------------------------------------------------------------------------
// detect_duplicates
// ---------------------------------------------------------------------------

#[test]
fn test_detect_duplicates_none() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(&a).expect("mkdir");
    std::fs::create_dir_all(&b).expect("mkdir");
    write_toml(&a, r#"name = "alpha""#);
    write_toml(&b, r#"name = "beta""#);

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let pa = loader
        .load_from_dir(&a, PluginLoadSource::SessionDir, None)
        .expect("load");
    let pb = loader
        .load_from_dir(&b, PluginLoadSource::SessionDir, None)
        .expect("load");

    let errors = PluginLoader::detect_duplicates(&[pa, pb]);
    assert!(errors.is_empty());
}

#[test]
fn test_detect_duplicates_found() {
    let id = PluginId {
        name: "dup".to_string(),
        marketplace: "inline".to_string(),
    };
    let manifest = PluginManifestV2 {
        name: "dup".to_string(),
        version: None,
        description: None,
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: None,
        dependencies: None,
        skills: None,
        hooks: None,
        agents: None,
        commands: None,
        mcp_servers: None,
        lsp_servers: None,
        output_styles: None,
        channels: None,
        user_config: None,
        settings: None,
        env_vars: None,
        min_version: None,
        max_version: None,
    };
    let p1 = LoadedPluginV2 {
        id: id.clone(),
        manifest: manifest.clone(),
        path: "/a".into(),
        load_source: PluginLoadSource::SessionDir,
        enabled: true,
    };
    let p2 = LoadedPluginV2 {
        id,
        manifest,
        path: "/b".into(),
        load_source: PluginLoadSource::SessionDir,
        enabled: true,
    };

    let errors = PluginLoader::detect_duplicates(&[p1, p2]);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("duplicate"));
}

// ---------------------------------------------------------------------------
// record_installation
// ---------------------------------------------------------------------------

#[test]
fn test_record_installation() {
    let plugin = LoadedPluginV2 {
        id: PluginId {
            name: "test".to_string(),
            marketplace: "mkt".to_string(),
        },
        manifest: PluginManifestV2 {
            name: "test".to_string(),
            version: Some("1.2.3".to_string()),
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: None,
            dependencies: None,
            skills: None,
            hooks: None,
            agents: None,
            commands: None,
            mcp_servers: None,
            lsp_servers: None,
            output_styles: None,
            channels: None,
            user_config: None,
            settings: None,
            env_vars: None,
            min_version: None,
            max_version: None,
        },
        path: "/cache/mkt/test".into(),
        load_source: PluginLoadSource::Marketplace {
            marketplace: "mkt".to_string(),
        },
        enabled: true,
    };

    let record =
        PluginLoader::record_installation(&plugin, Some("https://example.com".to_string()));

    assert_eq!(record.name, "test");
    assert_eq!(record.version, "1.2.3");
    assert_eq!(record.scope, PluginScope::User);
    assert_eq!(record.source_url.as_deref(), Some("https://example.com"));
    // installed_at should be a recent ISO timestamp
    assert!(record.installed_at.contains('T'));
}

// ---------------------------------------------------------------------------
// discover_contributions
// ---------------------------------------------------------------------------

#[test]
fn test_discover_contributions_from_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_dir = tmp.path().join("contrib-plugin");
    std::fs::create_dir_all(plugin_dir.join("skills")).expect("mkdir");
    std::fs::create_dir_all(plugin_dir.join("agents")).expect("mkdir");
    std::fs::create_dir_all(plugin_dir.join("commands")).expect("mkdir");

    std::fs::write(plugin_dir.join("PLUGIN.toml"), r#"name = "contrib-plugin""#).expect("write");
    std::fs::write(plugin_dir.join("skills").join("search.md"), "# search").expect("write");
    std::fs::write(plugin_dir.join("agents").join("reviewer.md"), "# reviewer").expect("write");
    std::fs::write(plugin_dir.join("commands").join("deploy.md"), "# deploy").expect("write");

    let loader = PluginLoader::new(tmp.path().to_path_buf());
    let plugin = loader
        .load_from_dir(&plugin_dir, PluginLoadSource::SessionDir, None)
        .expect("load");

    let contrib = PluginLoader::discover_contributions(&plugin);
    assert!(contrib.skills.contains(&"search".to_string()));
    assert!(contrib.agents.contains(&"reviewer".to_string()));
    assert!(contrib.commands.contains(&"deploy".to_string()));
}

// ---------------------------------------------------------------------------
// Dependency resolution
// ---------------------------------------------------------------------------

#[test]
fn test_verify_and_demote_unsatisfied_deps() {
    let mk = |name: &str, mkt: &str, deps: Option<Vec<&str>>| LoadedPluginV2 {
        id: PluginId {
            name: name.to_string(),
            marketplace: mkt.to_string(),
        },
        manifest: PluginManifestV2 {
            name: name.to_string(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: None,
            dependencies: deps.map(|d| d.into_iter().map(String::from).collect()),
            skills: None,
            hooks: None,
            agents: None,
            commands: None,
            mcp_servers: None,
            lsp_servers: None,
            output_styles: None,
            channels: None,
            user_config: None,
            settings: None,
            env_vars: None,
            min_version: None,
            max_version: None,
        },
        path: "/tmp".into(),
        load_source: PluginLoadSource::Marketplace {
            marketplace: mkt.to_string(),
        },
        enabled: true,
    };

    // "needs-dep" depends on "missing-dep" which is not loaded.
    let plugins = vec![
        mk("standalone", "mkt", None),
        mk("needs-dep", "mkt", Some(vec!["missing-dep"])),
    ];

    let demoted = verify_and_demote(&plugins);
    assert!(demoted.contains("needs-dep@mkt"));
    assert!(!demoted.contains("standalone@mkt"));
}

// ---------------------------------------------------------------------------
// InstalledPluginsManager
// ---------------------------------------------------------------------------

#[test]
fn test_installed_plugins_manager_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("installed_plugins.json");

    let mut mgr = InstalledPluginsManager::load(path.clone()).expect("load empty");
    assert!(!mgr.is_installed("test@mkt"));

    mgr.record_installation(
        "test@mkt",
        PluginInstallationEntry {
            scope: PluginScope::User,
            project_path: None,
            install_path: "/cache/test".to_string(),
            version: Some("1.0.0".to_string()),
            installed_at: Some("2024-01-01T00:00:00Z".to_string()),
            last_updated: None,
            git_commit_sha: None,
        },
    );
    assert!(mgr.is_installed("test@mkt"));
    assert_eq!(mgr.installed_plugin_ids().len(), 1);

    mgr.save().expect("save");

    // Reload and verify
    let reloaded = InstalledPluginsManager::load(path).expect("reload");
    assert!(reloaded.is_installed("test@mkt"));
    assert_eq!(reloaded.get_installations("test@mkt").len(), 1);
}

// ---------------------------------------------------------------------------
// resolve_cache_path (version segment) + load_all_plugins orchestrator
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_cache_path_includes_version_segment() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loader = PluginLoader::new(tmp.path().to_path_buf());
    // Installer writes cache/{mkt}/{plugin}/{version}.
    let versioned = tmp
        .path()
        .join("cache")
        .join("mymkt")
        .join("plug")
        .join("2.0.0");
    std::fs::create_dir_all(&versioned).expect("mkdir");

    // Exact requested version resolves to the version dir.
    assert_eq!(
        loader.resolve_cache_path("mymkt", "plug", Some("2.0.0")),
        versioned
    );
    // No version → newest subdir (was the bug: returned the parent, one level too shallow).
    assert_eq!(loader.resolve_cache_path("mymkt", "plug", None), versioned);
}

#[test]
fn test_load_all_plugins_inline_disabled_and_reserved_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loader = PluginLoader::new(tmp.path().to_path_buf());

    let mk = |name: &str| {
        let d = tmp.path().join(name);
        std::fs::create_dir_all(&d).expect("mkdir");
        write_toml(&d, &format!("name = \"{name}\"\ndescription = \"d\"\n"));
        d
    };
    let foo = mk("foo");
    let bar = mk("bar");
    // A reserved infra dir and a non-plugin dir must be skipped (not errors).
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&cache).expect("mkdir");
    let plain = tmp.path().join("not-a-plugin");
    std::fs::create_dir_all(&plain).expect("mkdir");

    let disabled: HashSet<String> = ["bar@inline".to_string()].into_iter().collect();
    let result =
        loader.load_all_plugins(&[foo, bar, cache, plain], &[], &HashSet::new(), &disabled);

    assert!(
        result.errors.is_empty(),
        "reserved/non-plugin dirs must not error: {:?}",
        result.errors
    );
    let by_id = |id: &str| result.plugins.iter().find(|p| p.id.as_str() == id);
    assert!(
        by_id("foo@inline").is_some_and(|p| p.enabled),
        "foo enabled"
    );
    assert!(
        by_id("bar@inline").is_some_and(|p| !p.enabled),
        "bar disabled via disabled_ids"
    );
}
