use super::*;

#[test]
fn nested_config_write_rejects_scalar_parent_without_mutating_it() {
    let mut doc = serde_json::json!({ "permissions": "locked" });

    let error = set_nested_json_key(
        &mut doc,
        "permissions.default_mode",
        serde_json::json!("default"),
    )
    .expect_err("scalar parent must be rejected");

    assert!(error.contains("permissions"));
    assert_eq!(doc, serde_json::json!({ "permissions": "locked" }));
}

#[test]
fn nested_config_write_rejects_non_object_root() {
    let mut doc = serde_json::json!(["existing"]);

    set_nested_json_key(&mut doc, "theme", serde_json::json!("dark"))
        .expect_err("non-object root must be rejected");

    assert_eq!(doc, serde_json::json!(["existing"]));
}

#[test]
fn process_config_read_observes_external_file_edits() {
    let dir = tempfile::TempDir::new().unwrap();
    let cwd = dir.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let roots = crate::paths::settings_roots_for_cwd(&cwd);
    let settings_path = coco_config::global_config::project_settings_path(roots.project_root());
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let catalogs = coco_config::CatalogPaths::empty_in(dir.path().join("home"));
    let enabled = coco_config::parse_enabled_setting_sources(None);

    std::fs::write(&settings_path, r#"{ "show_thinking": false }"#).unwrap();
    let first = load_process_settings_from_disk(&cwd, None, &enabled, &catalogs).unwrap();
    assert!(!first.merged.show_thinking);

    std::fs::write(&settings_path, r#"{ "show_thinking": true }"#).unwrap();
    let second = load_process_settings_from_disk(&cwd, None, &enabled, &catalogs).unwrap();
    assert!(second.merged.show_thinking);
}
