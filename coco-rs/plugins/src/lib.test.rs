use super::*;

#[test]
fn test_get_plugin_dirs_includes_both_config_and_project() {
    // The host calls `get_plugin_dirs(config_dir, project_dir)` at
    // startup — the result is the loader's input. Verifies both
    // user-level (`<config_dir>/plugins/*/`) and project-level
    // (`<project_dir>/.coco/plugins/*/`) directories are surfaced.
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path().join("config");
    let project_dir = tmp.path().join("project");
    let user_plugin = config_dir.join("plugins").join("user-plug");
    let proj_plugin = project_dir.join(".coco").join("plugins").join("proj-plug");
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
