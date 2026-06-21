use coco_types::PermissionMode;
use coco_types::ProviderModelSelection;

use super::*;

fn settings_with_main(model_id: &str) -> Settings {
    Settings {
        models: crate::ModelSelectionSettings {
            main: Some(crate::RoleSlots::new(ProviderModelSelection {
                provider: "anthropic".into(),
                model_id: model_id.into(),
            })),
            ..Default::default()
        },
        ..Default::default()
    }
}

// ── validate_permission_rule_string ──

#[test]
fn test_valid_simple_tool_rules() {
    assert!(validate_permission_rule_string("Read").is_ok());
    assert!(validate_permission_rule_string("Bash").is_ok());
    assert!(validate_permission_rule_string("Bash(git status)").is_ok());
    assert!(validate_permission_rule_string("Bash(npm:*)").is_ok());
    assert!(validate_permission_rule_string("Edit(*.rs)").is_ok());
    assert!(validate_permission_rule_string("mcp__server1").is_ok());
    assert!(validate_permission_rule_string("mcp__server1__tool1").is_ok());
    assert!(validate_permission_rule_string("mcp__server1__*").is_ok());
}

#[test]
fn test_empty_rule_rejected() {
    assert!(validate_permission_rule_string("").is_err());
    assert!(validate_permission_rule_string("   ").is_err());
}

#[test]
fn test_mismatched_parens_rejected() {
    assert!(validate_permission_rule_string("Bash(git *").is_err());
    assert!(validate_permission_rule_string("Bash)").is_err());
}

#[test]
fn test_empty_parens_rejected() {
    let err = validate_permission_rule_string("Bash()").unwrap_err();
    assert!(err.contains("Empty parentheses"), "got: {err}");
}

#[test]
fn test_lowercase_tool_name_rejected() {
    let err = validate_permission_rule_string("bash(git *)").unwrap_err();
    assert!(err.contains("uppercase"), "got: {err}");
}

#[test]
fn test_mcp_with_parens_rejected() {
    let err = validate_permission_rule_string("mcp__server(pattern)").unwrap_err();
    assert!(err.contains("MCP"), "got: {err}");
}

#[test]
fn test_bash_colon_star_misplaced_rejected() {
    let err = validate_permission_rule_string("Bash(npm:*:extra)").unwrap_err();
    assert!(err.contains(":*"), "got: {err}");
}

#[test]
fn test_bash_empty_prefix_before_colon_star() {
    let err = validate_permission_rule_string("Bash(:*)").unwrap_err();
    assert!(err.contains("Prefix cannot be empty"), "got: {err}");
}

// ── validate_permission_rules (string-based) ──

#[test]
fn test_validate_permissions_no_errors_on_valid() {
    let config = PermissionsConfig {
        allow: vec!["Read".into()],
        deny: vec![],
        ask: vec![],
        ..Default::default()
    };
    let errors = validate_permission_rules(&config);
    assert!(errors.is_empty(), "expected no errors: {errors:?}");
}

#[test]
fn test_validate_permissions_empty_tool_name() {
    let config = PermissionsConfig {
        allow: vec!["".into()],
        deny: vec![],
        ask: vec![],
        ..Default::default()
    };
    let errors = validate_permission_rules(&config);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].message.contains("empty"),
        "got: {}",
        errors[0].message
    );
}

#[test]
fn test_validate_permissions_conflict_detected() {
    let config = PermissionsConfig {
        allow: vec!["Bash(git *)".into()],
        deny: vec!["Bash(git *)".into()],
        ask: vec![],
        ..Default::default()
    };
    let errors = validate_permission_rules(&config);
    assert!(
        errors.iter().any(|e| e.message.contains("allow and deny")),
        "expected conflict: {errors:?}"
    );
}

// ── validate_settings ──

#[test]
fn test_validate_settings_bypass_conflict() {
    let settings = Settings {
        permissions: PermissionsConfig {
            default_mode: Some(PermissionMode::BypassPermissions),
            disable_bypass_mode: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let errors = validate_settings(&settings);
    assert!(
        errors.iter().any(|e| e.message.contains("conflicts")),
        "expected conflict error: {errors:?}"
    );
}

#[test]
fn test_validate_settings_model_not_in_allowlist() {
    let settings = Settings {
        available_models: Some(vec!["haiku".into()]),
        ..settings_with_main("claude-opus-4-6")
    };
    let errors = validate_settings(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("not in the available_models")),
        "expected model error: {errors:?}"
    );
}

#[test]
fn test_validate_settings_model_matches_family_alias() {
    let settings = Settings {
        available_models: Some(vec!["opus".into()]),
        ..settings_with_main("claude-opus-4-6")
    };
    let errors = validate_settings(&settings);
    // "opus" should match "claude-opus-4-6" as a family alias
    let model_errors: Vec<_> = errors.iter().filter(|e| e.path == "models.main").collect();
    assert!(
        model_errors.is_empty(),
        "should not flag opus match: {model_errors:?}"
    );
}

#[test]
fn test_validate_settings_empty_available_models_denies_selected_model() {
    let settings = Settings {
        available_models: Some(Vec::new()),
        ..settings_with_main("claude-opus-4-6")
    };
    let errors = validate_settings(&settings);
    assert!(
        errors.iter().any(|e| e.path == "models.main"),
        "expected model allowlist error: {errors:?}"
    );
}

#[test]
fn test_validate_settings_auto_mode_empty_string() {
    let settings = Settings {
        auto_mode: Some(crate::settings::AutoModeConfig {
            allow: vec!["".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let errors = validate_settings(&settings);
    assert!(
        errors.iter().any(|e| e.message.contains("Empty string")),
        "expected empty string error: {errors:?}"
    );
}

// ── is_setting_supported ──

#[test]
fn test_known_fields() {
    assert!(is_setting_supported("permissions"));
    assert!(is_setting_supported("hooks"));
    assert!(is_setting_supported("env"));
    assert!(is_setting_supported("log"));
}

#[test]
fn test_unknown_fields() {
    assert!(!is_setting_supported("nonexistent_field"));
    assert!(!is_setting_supported("foo_bar"));
}

// ── filter_invalid_permission_rules ──

#[test]
fn test_filter_invalid_rules_removes_non_strings() {
    let mut data = serde_json::json!({
        "permissions": {
            "allow": ["Read", 42, "Bash(git *)"],
            "deny": ["Write"]
        }
    });
    let warnings = filter_invalid_permission_rules(&mut data, "test.json");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("Non-string"));

    // The numeric value should be removed
    let allow = data["permissions"]["allow"].as_array().expect("array");
    assert_eq!(allow.len(), 2);
}

#[test]
fn test_filter_invalid_rules_removes_bad_syntax() {
    let mut data = serde_json::json!({
        "permissions": {
            "allow": ["Read", "bash(invalid)"],
        }
    });
    let warnings = filter_invalid_permission_rules(&mut data, "test.json");
    assert_eq!(
        warnings.len(),
        1,
        "should warn about lowercase: {warnings:?}"
    );

    let allow = data["permissions"]["allow"].as_array().expect("array");
    assert_eq!(allow.len(), 1);
    assert_eq!(allow[0].as_str(), Some("Read"));
}

#[test]
fn test_filter_no_permissions_section() {
    let mut data = serde_json::json!({"language": "en"});
    let warnings = filter_invalid_permission_rules(&mut data, "test.json");
    assert!(warnings.is_empty());
}

// ── is_escaped / has_unescaped_empty_parens ──

#[test]
fn test_escaped_paren_not_counted() {
    assert_eq!(count_unescaped(r"Bash\(git\)", '('), 0);
    assert_eq!(count_unescaped("Bash(git)", '('), 1);
}

#[test]
fn test_has_unescaped_empty_parens_positive() {
    assert!(has_unescaped_empty_parens("Bash()"));
}

#[test]
fn test_has_unescaped_empty_parens_escaped() {
    assert!(!has_unescaped_empty_parens(r"Bash\()"));
}

#[test]
fn test_has_unescaped_empty_parens_with_content() {
    assert!(!has_unescaped_empty_parens("Bash(git *)"));
}

// ── validate_mcp_configs ──

#[test]
fn test_mcp_valid_config() {
    let settings = Settings {
        allowed_mcp_servers: vec![crate::settings::AllowedMcpServerEntry {
            name: "my-server".into(),
            config: Some(serde_json::json!({"url": "http://localhost:3000"})),
        }],
        ..Default::default()
    };
    let errors = validate_mcp_configs(&settings);
    assert!(errors.is_empty(), "expected no errors: {errors:?}");
}

#[test]
fn test_mcp_empty_server_name() {
    let settings = Settings {
        allowed_mcp_servers: vec![crate::settings::AllowedMcpServerEntry {
            name: "".into(),
            config: None,
        }],
        ..Default::default()
    };
    let errors = validate_mcp_configs(&settings);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("cannot be empty"));
}

#[test]
fn test_mcp_duplicate_allowed_servers() {
    let settings = Settings {
        allowed_mcp_servers: vec![
            crate::settings::AllowedMcpServerEntry {
                name: "server-a".into(),
                config: None,
            },
            crate::settings::AllowedMcpServerEntry {
                name: "server-a".into(),
                config: None,
            },
        ],
        ..Default::default()
    };
    let errors = validate_mcp_configs(&settings);
    assert!(
        errors.iter().any(|e| e.message.contains("Duplicate")),
        "expected duplicate error: {errors:?}"
    );
}

#[test]
fn test_mcp_server_in_both_lists() {
    let settings = Settings {
        allowed_mcp_servers: vec![crate::settings::AllowedMcpServerEntry {
            name: "server-x".into(),
            config: None,
        }],
        denied_mcp_servers: vec![crate::settings::DeniedMcpServerEntry {
            name: "server-x".into(),
        }],
        ..Default::default()
    };
    let errors = validate_mcp_configs(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("allowed and denied")),
        "expected conflict: {errors:?}"
    );
}

#[test]
fn test_mcp_config_not_object() {
    let settings = Settings {
        allowed_mcp_servers: vec![crate::settings::AllowedMcpServerEntry {
            name: "server".into(),
            config: Some(serde_json::json!("not an object")),
        }],
        ..Default::default()
    };
    let errors = validate_mcp_configs(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("must be a JSON object")),
        "expected type error: {errors:?}"
    );
}

// ── validate_hooks ──

#[test]
fn test_hooks_valid() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "PreToolUse": {
                "type": "command",
                "command": "echo hello"
            }
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(errors.is_empty(), "expected no errors: {errors:?}");
}

#[test]
fn test_hooks_not_object() {
    let settings = Settings {
        hooks: Some(serde_json::json!("invalid")),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("must be an object"));
}

#[test]
fn test_hooks_unknown_event_type() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "UnknownEvent": {"type": "command", "command": "echo"}
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Unknown hook event type")),
        "expected unknown event error: {errors:?}"
    );
}

#[test]
fn test_hooks_missing_type_field() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "PostToolUse": {"command": "echo"}
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("missing required 'type'")),
        "expected missing type: {errors:?}"
    );
}

#[test]
fn test_hooks_command_type_missing_command() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "PreToolUse": {"type": "command"}
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("missing required 'command'")),
        "expected missing command: {errors:?}"
    );
}

#[test]
fn test_hooks_webhook_type_missing_url() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "SessionStart": {"type": "webhook"}
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("missing required 'url'")),
        "expected missing url: {errors:?}"
    );
}

#[test]
fn test_hooks_array_of_definitions() {
    let settings = Settings {
        hooks: Some(serde_json::json!({
            "PreToolUse": [
                {"type": "command", "command": "lint"},
                {"type": "prompt", "prompt": "Check safety"}
            ]
        })),
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(errors.is_empty(), "expected no errors: {errors:?}");
}

#[test]
fn test_hooks_none_returns_empty() {
    let settings = Settings {
        hooks: None,
        ..Default::default()
    };
    let errors = validate_hooks(&settings);
    assert!(errors.is_empty());
}

// ── validate_providers ──

#[test]
fn test_validate_providers_plaintext_key_with_env_key() {
    let mut settings = Settings::default();
    settings.providers.insert(
        "acme".into(),
        crate::provider::PartialProviderConfig {
            env_key: Some("ACME_API_KEY".into()),
            api_key: Some(crate::secret::RedactedSecret::new("sk-secret")),
            ..Default::default()
        },
    );
    let errors = validate_providers(&settings);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("`ACME_API_KEY`"));
    assert!(
        errors[0]
            .suggestion
            .as_deref()
            .unwrap()
            .contains("ACME_API_KEY=...")
    );
}

#[test]
fn test_validate_providers_plaintext_key_without_env_key_no_empty_backticks() {
    let mut settings = Settings::default();
    settings.providers.insert(
        "acme".into(),
        crate::provider::PartialProviderConfig {
            env_key: None,
            api_key: Some(crate::secret::RedactedSecret::new("sk-secret")),
            ..Default::default()
        },
    );
    let errors = validate_providers(&settings);
    assert_eq!(errors.len(), 1);
    // The malformed empty-backtick form must never appear in EITHER field.
    assert!(
        !errors[0].message.contains("``"),
        "message must not render empty backticks: {}",
        errors[0].message
    );
    assert!(errors[0].message.contains("<provider env_key>"));
    assert!(
        errors[0]
            .suggestion
            .as_deref()
            .unwrap()
            .contains("<provider env_key>")
    );
}
