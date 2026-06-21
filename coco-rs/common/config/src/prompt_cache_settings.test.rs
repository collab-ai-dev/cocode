use super::*;
use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;
use pretty_assertions::assert_eq;

fn empty_env() -> EnvSnapshot {
    EnvSnapshot::from_pairs(std::iter::empty::<(EnvKey, String)>())
}

fn empty_settings() -> Settings {
    Settings::default()
}

#[test]
fn prompt_cache_default_allowlist_is_empty() {
    let cfg = PromptCacheRuntimeConfig::resolve(&empty_settings(), &empty_env());
    assert!(cfg.allowlist.is_empty());
}

#[test]
fn prompt_cache_settings_allowlist_is_honored() {
    let mut s = empty_settings();
    s.prompt_cache.allowlist = Some(vec!["repl_main_thread".into(), "agent:*".into()]);
    let cfg = PromptCacheRuntimeConfig::resolve(&s, &empty_env());
    assert_eq!(cfg.allowlist, vec!["repl_main_thread", "agent:*"]);
}

#[test]
fn prompt_cache_env_allowlist_overrides_settings() {
    let mut s = empty_settings();
    s.prompt_cache.allowlist = Some(vec!["from_settings".into()]);
    let env = EnvSnapshot::from_pairs([(
        EnvKey::CocoPromptCacheAllowlist,
        "from_env_a, from_env_b ,from_env_c",
    )]);
    let cfg = PromptCacheRuntimeConfig::resolve(&s, &env);
    assert_eq!(
        cfg.allowlist,
        vec!["from_env_a", "from_env_b", "from_env_c"]
    );
}

#[test]
fn account_defaults_to_api_key_no_overage() {
    let cfg = AccountConfig::resolve(&empty_settings(), &empty_env());
    assert_eq!(cfg.kind, coco_types::AccountKind::ApiKey);
    assert!(!cfg.in_overage);
}

#[test]
fn account_settings_subscriber_in_overage() {
    let mut s = empty_settings();
    s.account.kind = Some(AccountKindSetting::ClaudeAiSubscriber);
    s.account.in_overage = Some(true);
    let cfg = AccountConfig::resolve(&s, &empty_env());
    assert_eq!(cfg.kind, coco_types::AccountKind::ClaudeAiSubscriber);
    assert!(cfg.in_overage);
}

#[test]
fn account_kind_setting_serde_uses_snake_case() {
    let json = serde_json::to_string(&AccountKindSetting::ClaudeAiSubscriber).unwrap();
    assert_eq!(json, "\"claude_ai_subscriber\"");
}
