use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

#[test]
fn test_parse_settings_accepts_jsonc_comments_and_trailing_commas() {
    let settings = parse_settings(
        r#"{
            // JSONC is accepted in settings.json-shaped content.
            language: "zh-CN",
            "features": {
                "web_search": true,
            },
        }"#,
    )
    .expect("parse JSONC settings");

    assert_eq!(settings.language.as_deref(), Some("zh-CN"));
    assert_eq!(settings.features.get("web_search"), Some(&true));
}

#[test]
fn test_parse_settings_rejects_top_level_model() {
    let err = parse_settings(r#"{ "model": "openai/gpt-5-5" }"#)
        .expect_err("top-level model is not supported");

    assert!(err.to_string().contains("models.main"), "got: {err}");
}

#[test]
fn test_parse_settings_rejects_unknown_top_level_key() {
    let err = parse_settings(r#"{ "not_a_real_setting": true }"#)
        .expect_err("unknown top-level key is not supported");

    assert!(err.to_string().contains("not_a_real_setting"), "got: {err}");
}

#[test]
fn test_parse_settings_accepts_ts_permission_policy_key() {
    let settings = parse_settings(
        r#"{
            "permissions": {
                "allowManagedPermissionRulesOnly": true
            }
        }"#,
    )
    .expect("parse settings");

    assert!(settings.permissions.allow_managed_permission_rules_only);
}

#[test]
fn test_plan_mode_clear_context_default_is_enabled() {
    let settings = parse_settings("{}").expect("parse empty settings");
    assert!(settings.plan_mode.show_clear_context_on_exit);
}

#[test]
fn test_plan_mode_verify_execution_default_is_disabled() {
    let settings = parse_settings("{}").expect("parse empty settings");
    assert!(!settings.plan_mode.verify_execution);
}

#[test]
fn test_plan_mode_clear_context_can_be_disabled() {
    let settings = parse_settings(
        r#"{
            "plan_mode": {
                "show_clear_context_on_exit": false
            }
        }"#,
    )
    .expect("parse settings");

    assert!(!settings.plan_mode.show_clear_context_on_exit);
}

#[test]
fn test_parse_settings_accepts_tui_native_replay_cache_policy() {
    let settings = parse_settings(
        r#"{
            "tui": {
                "native_replay_cache": {
                    "enabled": false,
                    "max_entries": 7,
                    "max_estimated_kb": 128,
                    "min_cells": 3,
                    "min_content_kb": 4,
                    "admit_min_render_us": 99,
                    "admit_min_result_kb": 5
                }
            }
        }"#,
    )
    .expect("parse TUI settings");

    let cache = settings.tui.native_replay_cache;
    assert!(!cache.enabled);
    assert_eq!(cache.max_entries, 7);
    assert_eq!(cache.max_estimated_kb, 128);
    assert_eq!(cache.min_cells, 3);
    assert_eq!(cache.min_content_kb, 4);
    assert_eq!(cache.admit_min_render_us, 99);
    assert_eq!(cache.admit_min_result_kb, 5);
}

#[test]
fn test_parse_settings_accepts_tui_performance_policy() {
    let settings = parse_settings(
        r#"{
            "tui": {
                "performance": {
                    "enabled": true,
                    "sample_every_n_frames": 7,
                    "slow_frame_ms": 33,
                    "slow_stage_us": 750
                }
            }
        }"#,
    )
    .expect("parse TUI settings");

    let performance = settings.tui.performance;
    assert!(performance.enabled);
    assert_eq!(performance.sample_every_n_frames, 7);
    assert_eq!(performance.slow_frame_ms, 33);
    assert_eq!(performance.slow_stage_us, 750);
}

#[test]
fn test_parse_settings_accepts_status_line_camel_case() {
    let settings = parse_settings(
        r#"{
            "statusLine": {
                "type": "command",
                "command": "printf ok",
                "padding": 1
            }
        }"#,
    )
    .expect("parse statusLine settings");

    let status_line = settings.status_line.expect("statusLine parsed");
    let command = status_line.as_command();
    assert_eq!(command.command, "printf ok");
    assert_eq!(command.padding, 1);
}

#[test]
fn test_parse_settings_accepts_status_line_snake_case_alias() {
    let settings = parse_settings(
        r#"{
            "status_line": {
                "type": "command",
                "command": "printf snake"
            }
        }"#,
    )
    .expect("parse status_line settings");

    assert_eq!(
        settings
            .status_line
            .expect("status_line parsed")
            .as_command()
            .command,
        "printf snake"
    );
}

#[test]
fn test_parse_settings_rejects_unknown_status_line_type() {
    let err = parse_settings(
        r#"{
            "statusLine": {
                "type": "template",
                "command": "ignored"
            }
        }"#,
    )
    .expect_err("unknown statusLine type should fail");

    assert!(err.to_string().contains("template"));
}

#[test]
fn test_load_settings_with_accepts_jsonc_layers() {
    let tmp = TempDir::new().expect("tempdir");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(cwd.join(".coco")).expect("project settings dir");

    let user_path = tmp.path().join("settings.json");
    let managed_path = tmp.path().join("managed-settings.json");
    let flag_path = tmp.path().join("flag-settings.json");

    std::fs::write(
        &user_path,
        r#"{
            "language": "en",
            "features": {
                "web_search": true,
            },
        }"#,
    )
    .expect("write user settings");
    std::fs::write(
        cwd.join(".coco/settings.json"),
        r#"{
            // Project settings can also use comments.
            "output_style": "project",
        }"#,
    )
    .expect("write project settings");
    std::fs::write(
        &flag_path,
        r#"{
            "language": "fr",
        }"#,
    )
    .expect("write flag settings");
    std::fs::write(
        &managed_path,
        r#"{
            "features": {
                "web_fetch": true,
            },
        }"#,
    )
    .expect("write managed settings");

    let settings = load_settings_with(
        &cwd,
        Some(&flag_path),
        &user_path,
        &managed_path,
        &all_setting_sources(),
    )
    .expect("load JSONC settings");

    assert_eq!(settings.merged.language.as_deref(), Some("fr"));
    assert_eq!(settings.merged.output_style.as_deref(), Some("project"));
    assert_eq!(settings.merged.features.get("web_search"), Some(&true));
    assert_eq!(settings.merged.features.get("web_fetch"), Some(&true));
    assert!(settings.per_source.contains_key(&SettingSource::User));
    assert!(settings.per_source.contains_key(&SettingSource::Project));
    assert!(settings.per_source.contains_key(&SettingSource::Flag));
    assert!(settings.per_source.contains_key(&SettingSource::Policy));
}

#[test]
fn test_strict_plugin_only_customization_serde() {
    // `true` → AllLocked(true); locks every surface.
    let s: Settings =
        serde_json::from_str(r#"{"strict_plugin_only_customization": true}"#).expect("true");
    assert_eq!(
        s.strict_plugin_only_customization,
        StrictPluginOnlyCustomization::AllLocked(true)
    );
    assert!(
        s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("skills")
    );

    // `false` → AllLocked(false); locks nothing.
    let s: Settings =
        serde_json::from_str(r#"{"strict_plugin_only_customization": false}"#).expect("false");
    assert_eq!(
        s.strict_plugin_only_customization,
        StrictPluginOnlyCustomization::AllLocked(false)
    );
    assert!(
        !s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("skills")
    );

    // Array → SurfacesLocked; only the listed surfaces are locked.
    let s: Settings =
        serde_json::from_str(r#"{"strict_plugin_only_customization": ["skills", "mcp"]}"#)
            .expect("array");
    assert_eq!(
        s.strict_plugin_only_customization,
        StrictPluginOnlyCustomization::SurfacesLocked(vec!["skills".into(), "mcp".into()])
    );
    assert!(
        s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("skills")
    );
    assert!(
        s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("mcp")
    );
    assert!(
        !s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("agents")
    );

    // Absent → Disabled (the default); locks nothing.
    let s: Settings = serde_json::from_str(r#"{}"#).expect("absent");
    assert_eq!(
        s.strict_plugin_only_customization,
        StrictPluginOnlyCustomization::Disabled
    );
    assert!(
        !s.strict_plugin_only_customization
            .is_restricted_to_plugin_only("skills")
    );
}

#[test]
fn test_load_settings_with_skips_disabled_sources() {
    let tmp = TempDir::new().expect("tempdir");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(cwd.join(".coco")).expect("project settings dir");

    let user_path = tmp.path().join("settings.json");
    let managed_path = tmp.path().join("managed-settings.json");

    std::fs::write(&user_path, r#"{"output_style": "user"}"#).expect("write user settings");
    std::fs::write(
        cwd.join(".coco/settings.json"),
        r#"{"output_style": "project"}"#,
    )
    .expect("write project settings");
    std::fs::write(&managed_path, r#"{"strict_known_marketplaces": ["m"]}"#)
        .expect("write managed settings");

    // Only `project` enabled (plus the always-on Policy + Flag). The User
    // layer is skipped, so the project value wins and User is absent from
    // per_source.
    let enabled = crate::parse_enabled_setting_sources(Some("project"));
    let settings = load_settings_with(&cwd, None, &user_path, &managed_path, &enabled)
        .expect("load filtered settings");
    assert_eq!(settings.merged.output_style.as_deref(), Some("project"));
    assert!(!settings.per_source.contains_key(&SettingSource::User));
    assert!(settings.per_source.contains_key(&SettingSource::Project));
    // Policy always loads even when not named in the CSV.
    assert!(settings.per_source.contains_key(&SettingSource::Policy));
}
