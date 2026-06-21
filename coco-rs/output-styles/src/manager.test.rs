use super::*;
use crate::builtin::EXPLANATORY_STYLE_NAME;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn empty_manager_has_no_active_style_and_default_sdk_name() {
    let mgr = OutputStyleManager::empty();
    assert!(mgr.active().is_none());
    assert_eq!(mgr.active_name_for_sdk(), "default");
    assert!(mgr.names().is_empty());
}

#[test]
fn builder_with_default_settings_returns_none_active() {
    let mgr = OutputStyleManager::builder().build();
    assert!(mgr.active().is_none());
    assert_eq!(mgr.active_name_for_sdk(), "default");
    // Built-ins surface in the catalog.
    assert!(mgr.names().contains(&"Explanatory".to_string()));
    assert!(mgr.names().contains(&"Learning".to_string()));
}

#[test]
fn builder_resolves_built_in_explanatory() {
    let mgr = OutputStyleManager::builder()
        .settings_name(Some(EXPLANATORY_STYLE_NAME.to_string()))
        .build();
    let active = mgr.active().expect("Explanatory should be active");
    assert_eq!(active.name, "Explanatory");
    assert_eq!(mgr.active_name_for_sdk(), "Explanatory");
}

#[test]
fn project_dir_style_overrides_user_dir_style() {
    let user = tempdir().unwrap();
    std::fs::write(
        user.path().join("Concise.md"),
        "---\ndescription: USER\n---\nuser body\n",
    )
    .unwrap();
    let project = tempdir().unwrap();
    std::fs::write(
        project.path().join("Concise.md"),
        "---\ndescription: PROJECT\n---\nproject body\n",
    )
    .unwrap();

    let mgr = OutputStyleManager::builder()
        .settings_name(Some("Concise".to_string()))
        .user_dir(Some(user.path().to_path_buf()))
        .project_dirs(vec![project.path().to_path_buf()])
        .build();
    let active = mgr.active().expect("Concise should resolve");
    assert_eq!(active.description, "PROJECT");
    assert_eq!(active.source, OutputStyleSource::ProjectSettings);
}

#[test]
fn managed_dir_overrides_project() {
    let project = tempdir().unwrap();
    std::fs::write(
        project.path().join("Concise.md"),
        "---\ndescription: PROJECT\n---\nbody\n",
    )
    .unwrap();
    let managed = tempdir().unwrap();
    std::fs::write(
        managed.path().join("Concise.md"),
        "---\ndescription: POLICY\n---\nbody\n",
    )
    .unwrap();

    let mgr = OutputStyleManager::builder()
        .settings_name(Some("Concise".to_string()))
        .project_dirs(vec![project.path().to_path_buf()])
        .managed_dir(Some(managed.path().to_path_buf()))
        .build();
    let active = mgr.active().expect("Concise should resolve");
    assert_eq!(active.description, "POLICY");
    assert_eq!(active.source, OutputStyleSource::PolicySettings);
}

#[test]
fn duplicate_physical_dir_styles_are_deduped_in_ts_source_order() {
    let user = tempdir().unwrap();
    let project = tempdir().unwrap();
    let user_file = user.path().join("Concise.md");
    let project_file = project.path().join("Concise.md");
    std::fs::write(&user_file, "---\ndescription: USER\n---\nshared body\n").unwrap();
    std::fs::hard_link(&user_file, &project_file).unwrap();

    let mgr = OutputStyleManager::builder()
        .settings_name(Some("Concise".to_string()))
        .user_dir(Some(user.path().to_path_buf()))
        .project_dirs(vec![project.path().to_path_buf()])
        .build();

    let active = mgr.active().expect("Concise should resolve");
    assert_eq!(active.description, "USER");
    assert_eq!(active.source, OutputStyleSource::UserSettings);
}

#[test]
fn plugin_force_for_plugin_overrides_user_settings() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    std::fs::write(
        styles_dir.join("preferred.md"),
        "---\nforce-for-plugin: true\ndescription: from plugin\n---\nplugin body\n",
    )
    .unwrap();

    let mgr = OutputStyleManager::builder()
        .settings_name(Some(EXPLANATORY_STYLE_NAME.to_string()))
        .plugins(vec![crate::plugin_loader::PluginOutputStyleSource {
            plugin_name: "alpha".into(),
            default_dir: Some(styles_dir),
            extra_paths: vec![],
        }])
        .build();
    let active = mgr.active().unwrap();
    assert_eq!(active.name, "alpha:preferred");
    matches!(
        mgr.force_for_plugin_verdict(),
        ForceForPluginVerdict::Selected { .. }
    );
}
