use super::*;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRuleSource;
use coco_types::ToolName;
use coco_types::ToolPermissionContext;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

fn settings_with(per_source: HashMap<SettingSource, serde_json::Value>) -> SettingsWithSource {
    SettingsWithSource {
        merged: Settings::default(),
        per_source,
        source_paths: HashMap::new(),
    }
}

#[test]
fn permission_rule_source_roots_mirror_ts_settings_roots() {
    let original_cwd = Path::new("/repo");
    let mut settings = settings_with(HashMap::new());
    settings.source_paths.insert(
        SettingSource::User,
        PathBuf::from("/home/me/.coco/settings.json"),
    );
    settings.source_paths.insert(
        SettingSource::Flag,
        PathBuf::from("/tmp/coco-flags/custom.json"),
    );

    let roots = permission_rule_source_roots(&settings, original_cwd);

    assert_eq!(
        roots.get(&PermissionRuleSource::UserSettings),
        Some(&PathBuf::from("/home/me/.coco"))
    );
    assert_eq!(
        roots.get(&PermissionRuleSource::FlagSettings),
        Some(&PathBuf::from("/tmp/coco-flags"))
    );
    for source in [
        PermissionRuleSource::Session,
        PermissionRuleSource::Command,
        PermissionRuleSource::CliArg,
        PermissionRuleSource::ProjectSettings,
        PermissionRuleSource::LocalSettings,
        PermissionRuleSource::PolicySettings,
    ] {
        assert_eq!(roots.get(&source), Some(&PathBuf::from("/repo")));
    }
}

#[test]
fn maps_settings_sources_to_permission_sources_per_behavior() {
    let user_raw = serde_json::json!({
        "permissions": {
            "allow": ["WebFetch(domain:example.com)"],
            "deny":  ["Read(/etc/secret)"],
            "ask":   ["Bash(rm:*)"]
        }
    });
    let policy_raw = serde_json::json!({
        "permissions": {
            "allow": ["WebFetch(domain:enterprise.com)"]
        }
    });
    let mut per = HashMap::new();
    per.insert(SettingSource::User, user_raw);
    per.insert(SettingSource::Policy, policy_raw);
    let s = settings_with(per);

    let (allow, deny, ask) = typed_permission_rules(&s);

    let user_allow = allow.get(&PermissionRuleSource::UserSettings).unwrap();
    assert_eq!(user_allow.len(), 1);
    assert_eq!(user_allow[0].behavior, PermissionBehavior::Allow);
    assert_eq!(user_allow[0].value.tool_pattern, "WebFetch");
    assert_eq!(
        user_allow[0].value.rule_content.as_deref(),
        Some("domain:example.com")
    );

    let user_deny = deny.get(&PermissionRuleSource::UserSettings).unwrap();
    assert_eq!(user_deny.len(), 1);
    assert_eq!(user_deny[0].behavior, PermissionBehavior::Deny);

    let user_ask = ask.get(&PermissionRuleSource::UserSettings).unwrap();
    assert_eq!(user_ask.len(), 1);
    assert_eq!(user_ask[0].behavior, PermissionBehavior::Ask);
    assert_eq!(user_ask[0].value.tool_pattern, "Bash");

    let policy_allow = allow.get(&PermissionRuleSource::PolicySettings).unwrap();
    assert_eq!(policy_allow.len(), 1);
    assert!(
        !allow.contains_key(&PermissionRuleSource::Session),
        "settings conversion must not synthesize session rules"
    );
}

#[test]
fn drops_plugin_sourced_rules() {
    let plugin_raw = serde_json::json!({
        "permissions": { "allow": ["WebFetch(domain:plugin.com)"] }
    });
    let mut per = HashMap::new();
    per.insert(SettingSource::Plugin, plugin_raw);
    let s = settings_with(per);

    let (allow, deny, ask) = typed_permission_rules(&s);
    assert!(allow.is_empty(), "plugin allow rules dropped");
    assert!(
        !allow.contains_key(&PermissionRuleSource::ProjectSettings),
        "plugin allow rules dropped"
    );
    assert!(deny.is_empty());
    assert!(ask.is_empty());
}

#[test]
fn handles_missing_permissions_block_gracefully() {
    let raw = serde_json::json!({ "language": "en" });
    let mut per = HashMap::new();
    per.insert(SettingSource::User, raw);
    let s = settings_with(per);

    let (allow, deny, ask) = typed_permission_rules(&s);
    assert!(allow.is_empty());
    assert!(deny.is_empty());
    assert!(ask.is_empty());
}

#[test]
fn loaded_rules_do_not_require_permission_settings_for_read_only_tools() {
    let mut per = HashMap::new();
    per.insert(
        SettingSource::User,
        serde_json::json!({ "permissions": {} }),
    );
    let s = settings_with(per);
    let (allow, deny, ask) = typed_permission_rules(&s);
    let context = ToolPermissionContext {
        mode: PermissionMode::Default,
        additional_dirs: HashMap::new(),
        allow_rules: allow,
        deny_rules: deny,
        ask_rules: ask,
        bypass_available: false,
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        session_plan_file: None,
        permission_rule_source_roots: HashMap::new(),
    };

    let decision = coco_permissions::PermissionEvaluator::evaluate(
        &ToolName::Glob.into(),
        &serde_json::json!({"pattern": "**/*.rs"}),
        &context,
    );

    assert!(
        matches!(decision, coco_types::PermissionDecision::Allow { .. }),
        "read-only default behavior belongs to the evaluator, not the settings loader"
    );
}
