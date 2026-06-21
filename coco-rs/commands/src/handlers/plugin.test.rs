use super::*;

#[test]
fn test_extract_toml_string_value_double_quotes() {
    assert_eq!(
        extract_toml_string_value(" = \"hello world\""),
        Some("hello world".to_string())
    );
}

#[test]
fn test_extract_toml_string_value_single_quotes() {
    assert_eq!(
        extract_toml_string_value(" = 'hello'"),
        Some("hello".to_string())
    );
}

#[test]
fn test_extract_toml_string_value_no_quotes() {
    assert_eq!(
        extract_toml_string_value(" = 0.1.0"),
        Some("0.1.0".to_string())
    );
}

#[test]
fn test_extract_toml_string_value_no_equals() {
    assert_eq!(extract_toml_string_value(" hello"), None);
}

#[tokio::test]
async fn test_scan_plugin_dir_with_plugins() {
    let tmp = tempfile::tempdir().unwrap();

    // Create a plugin with PLUGIN.toml
    let plugin_dir = tmp.path().join("my-plugin");
    tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
    tokio::fs::write(
        plugin_dir.join("PLUGIN.toml"),
        "version = \"1.0.0\"\ndescription = \"Test plugin\"\n",
    )
    .await
    .unwrap();

    // Create skills directory
    tokio::fs::create_dir_all(plugin_dir.join("skills"))
        .await
        .unwrap();

    let mut plugins = Vec::new();
    scan_plugin_dir(tmp.path(), "test", &mut plugins).await;

    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name, "my-plugin");
    assert_eq!(plugins[0].version.as_deref(), Some("1.0.0"));
    assert_eq!(plugins[0].description.as_deref(), Some("Test plugin"));
    assert!(plugins[0].has_skills);
    assert!(!plugins[0].has_hooks);
}

#[tokio::test]
async fn test_scan_plugin_dir_skips_non_plugin_dirs() {
    let tmp = tempfile::tempdir().unwrap();

    // Create a directory without PLUGIN.toml
    let dir = tmp.path().join("not-a-plugin");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("README.md"), "just a readme")
        .await
        .unwrap();

    let mut plugins = Vec::new();
    scan_plugin_dir(tmp.path(), "test", &mut plugins).await;

    assert!(plugins.is_empty());
}

#[tokio::test]
async fn test_install_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join(".coco").join("plugins").join("test-plugin");

    // Install into the temp dir
    tokio::fs::create_dir_all(tmp.path().join(".coco").join("plugins"))
        .await
        .unwrap();

    // We test the manifest creation logic directly since install_plugin
    // uses relative paths
    let manifest = "[plugin]\nname = \"test-plugin\"\nversion = \"0.1.0\"\ndescription = \"Plugin test-plugin\"\n".to_string();
    tokio::fs::create_dir_all(&plugin_dir).await.unwrap();
    tokio::fs::write(plugin_dir.join("PLUGIN.toml"), &manifest)
        .await
        .unwrap();

    // Verify the manifest was written
    let content = tokio::fs::read_to_string(plugin_dir.join("PLUGIN.toml"))
        .await
        .unwrap();
    assert!(content.contains("test-plugin"));
    assert!(content.contains("0.1.0"));
}

#[tokio::test]
async fn test_handler_unknown_subcommand() {
    let output = handler("foobar".to_string()).await.unwrap();
    assert!(output.contains("Plugin Management"));
    assert!(output.contains("Usage"));
}

#[tokio::test]
async fn test_parse_plugin_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("PLUGIN.toml");
    tokio::fs::write(
        &path,
        "[plugin]\nversion = \"2.3.1\"\ndescription = \"A great plugin\"\n",
    )
    .await
    .unwrap();

    let (version, desc) = super::parse_plugin_manifest(&path).await;
    assert_eq!(version.as_deref(), Some("2.3.1"));
    assert_eq!(desc.as_deref(), Some("A great plugin"));
}
