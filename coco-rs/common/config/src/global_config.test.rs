use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[test]
fn test_write_user_setting_preserves_jsonc_comments() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
  // keep top-level comment
  "sandbox": {
    // keep nested comment
    "mode": "read_only",
  },
}
"#,
    )
    .expect("write settings");

    write_user_setting_at_path(
        &path,
        "sandbox.mode",
        serde_json::Value::String("workspace_write".to_string()),
    )
    .expect("write setting");

    let updated = std::fs::read_to_string(&path).expect("read settings");
    assert!(updated.contains("// keep top-level comment"));
    assert!(updated.contains("// keep nested comment"));
    assert!(updated.contains(r#""mode": "workspace_write""#));
    let settings = crate::settings::parse_settings(&updated).expect("parse updated settings");
    assert_eq!(
        settings.sandbox.mode,
        coco_types::SandboxMode::WorkspaceWrite
    );
}

#[test]
fn test_write_user_setting_appends_to_jsonc_file() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{
  // existing content remains in place
  "language": "en",
}
"#,
    )
    .expect("write settings");

    write_user_setting_at_path(&path, "features.web_search", serde_json::Value::Bool(true))
        .expect("write setting");

    let updated = std::fs::read_to_string(&path).expect("read settings");
    assert!(updated.contains("// existing content remains in place"));
    assert!(updated.contains(r#""language": "en""#));
    assert!(updated.contains(r#""features": {"#));
    assert!(updated.contains(r#""web_search": true"#));
    let settings = crate::settings::parse_settings(&updated).expect("parse updated settings");
    assert_eq!(settings.features.get("web_search"), Some(&true));
}

#[test]
fn test_write_user_setting_escapes_string_values() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    let raw_value = "C:\\Users\\me\\.env\n\"quoted\"";

    write_user_setting_at_path(
        &path,
        "paths.env_file",
        serde_json::Value::String(raw_value.to_string()),
    )
    .expect("write setting");

    let updated = std::fs::read_to_string(&path).expect("read settings");
    let parsed = crate::parse_jsonc_value(&updated).expect("parse updated settings");
    assert_eq!(parsed["paths"]["env_file"], raw_value);
}

#[test]
fn test_write_user_setting_parse_error_preserves_existing_file() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    let original = "{\n  // work in progress\n  \"language\": \"en\",\n";
    std::fs::write(&path, original).expect("write settings");

    let err = write_user_setting_at_path(
        &path,
        "language",
        serde_json::Value::String("en".to_string()),
    )
    .expect_err("invalid JSONC must not be overwritten");

    assert!(err.to_string().contains("jsonc error"));
    let updated = std::fs::read_to_string(&path).expect("read settings");
    assert_eq!(updated, original);
}

#[test]
fn test_write_user_setting_to_path_rejects_invalid_json_without_overwriting_file() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    let original = "{ invalid json";
    std::fs::write(&path, original).expect("write settings");

    let err =
        write_user_setting_to_path(&path, "language", json!("en")).expect_err("invalid settings");

    assert!(
        err.to_string().contains("jsonc error"),
        "unexpected error: {err}"
    );
    let updated = std::fs::read_to_string(&path).expect("read settings");
    assert_eq!(updated, original);
}

#[test]
fn test_write_user_setting_to_path_preserves_siblings_for_dotted_keys() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("settings.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "language": "en",
            "sandbox": {
                "mode": "workspace-write",
                "network": {
                    "mode": "full"
                }
            }
        }))
        .expect("serialize settings"),
    )
    .expect("write settings");

    let written =
        write_user_setting_to_path(&path, "sandbox.mode", json!("read_only")).expect("write");

    assert_eq!(written, path);
    let updated = std::fs::read_to_string(&path).expect("read settings");
    let value: serde_json::Value = serde_json::from_str(&updated).expect("parse settings");
    assert_eq!(
        value,
        json!({
            "language": "en",
            "sandbox": {
                "mode": "read_only",
                "network": {
                    "mode": "full"
                }
            }
        })
    );
}

#[test]
fn test_write_global_config_preserves_jsonc_comments() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("global.json");
    std::fs::write(
        &path,
        r#"{
  // keep global comment
  "projects": {
    // keep project comment
    "/repo": {
      "has_completed_project_onboarding": false
    }
  },
  "session_costs": {}
}
"#,
    )
    .expect("write global config");

    let mut config = GlobalConfig::default();
    config.projects.insert(
        "/repo".to_string(),
        ProjectConfig {
            has_completed_project_onboarding: true,
            ..Default::default()
        },
    );

    write_global_config_at_path(&path, &config).expect("write global config");

    let updated = std::fs::read_to_string(&path).expect("read global config");
    assert!(updated.contains("// keep global comment"));
    assert!(updated.contains("// keep project comment"));
    let parsed: GlobalConfig = crate::jsonc::from_str(&updated).expect("parse global config");
    assert!(
        parsed
            .projects
            .get("/repo")
            .expect("project")
            .has_completed_project_onboarding
    );
}
