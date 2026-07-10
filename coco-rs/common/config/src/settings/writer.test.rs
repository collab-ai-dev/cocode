//! Tests for [`LocalSettingsWriter`] and [`deep_merge_with_deletions`].

use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

// ─── deep_merge_with_deletions (pure) ───

#[test]
fn deletion_sentinel_removes_key() {
    let mut base = json!({ "skill_overrides": { "foo": "off", "bar": "on" } });
    let overlay = json!({ "skill_overrides": { "foo": null } });
    deep_merge_with_deletions(&mut base, &overlay);
    assert_eq!(base, json!({ "skill_overrides": { "bar": "on" } }));
}

#[test]
fn deletion_of_last_key_prunes_empty_parent() {
    let mut base = json!({ "skill_overrides": { "foo": "off" }, "other": 1 });
    let overlay = json!({ "skill_overrides": { "foo": null } });
    deep_merge_with_deletions(&mut base, &overlay);
    // `skill_overrides: {}` shouldn't linger
    assert_eq!(base, json!({ "other": 1 }));
}

#[test]
fn non_null_overlay_overwrites_leaf() {
    let mut base = json!({ "language": "en" });
    let overlay = json!({ "language": "zh" });
    deep_merge_with_deletions(&mut base, &overlay);
    assert_eq!(base, json!({ "language": "zh" }));
}

#[test]
fn nested_objects_merge_recursively() {
    let mut base = json!({ "a": { "b": 1, "c": 2 } });
    let overlay = json!({ "a": { "b": 9, "d": 3 } });
    deep_merge_with_deletions(&mut base, &overlay);
    assert_eq!(base, json!({ "a": { "b": 9, "c": 2, "d": 3 } }));
}

// ─── atomic write + read_or_default ───

#[test]
fn read_or_default_returns_empty_object_on_missing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nope.json");
    let v = read_or_default(&path).unwrap();
    assert_eq!(v, json!({}));
}

#[test]
fn read_or_default_returns_empty_object_on_empty_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.local.json");
    fs::write(&path, "  \n  ").unwrap();
    let v = read_or_default(&path).unwrap();
    assert_eq!(v, json!({}));
}

#[test]
fn atomic_write_creates_parent_dir() {
    let dir = TempDir::new().unwrap();
    let nested = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("settings.local.json");
    atomic_write(&nested, &json!({ "skill_overrides": { "foo": "off" } })).unwrap();
    let body = fs::read_to_string(&nested).unwrap();
    assert!(body.contains("skill_overrides"));
    assert!(body.contains("foo"));
}

#[test]
fn apply_patch_round_trip_with_deletion() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("settings.local.json");
    fs::write(
        &path,
        r#"{ "skill_overrides": { "alpha": "off", "beta": "name-only" } }"#,
    )
    .unwrap();

    apply_patch(&path, &json!({ "skill_overrides": { "alpha": null } })).unwrap();

    let body: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(body, json!({ "skill_overrides": { "beta": "name-only" } }));
}

#[tokio::test]
async fn explicit_roots_write_local_and_republish_project_settings() {
    let dir = TempDir::new().unwrap();
    let project_root = dir.path().join("project");
    let local_root = project_root.join("nested");
    let catalogs = CatalogPaths::empty_in(dir.path().join("home"));
    let project_settings = crate::global_config::project_settings_path(&project_root);
    fs::create_dir_all(project_settings.parent().unwrap()).unwrap();
    fs::create_dir_all(&local_root).unwrap();
    fs::write(&project_settings, r#"{ "language": "zh" }"#).unwrap();

    let initial = crate::RuntimeConfigBuilder::new(&local_root, EnvSnapshot::default())
        .with_settings_roots(&project_root, &local_root)
        .with_catalog_paths(catalogs.clone())
        .build()
        .unwrap();
    let publisher = Arc::new(RuntimePublisher::new(Arc::new(initial)));

    write_local_settings_with_roots(
        SettingsRoots::new(&project_root, &local_root),
        None,
        catalogs,
        Arc::clone(&publisher),
        json!({ "show_thinking": true }),
    )
    .await
    .unwrap();

    let local_settings = crate::global_config::local_settings_path(&local_root);
    assert!(local_settings.exists());
    assert!(!crate::global_config::local_settings_path(&project_root).exists());
    let current = publisher.current();
    assert_eq!(current.settings.merged.language.as_deref(), Some("zh"));
    assert!(current.settings.merged.show_thinking);
}
