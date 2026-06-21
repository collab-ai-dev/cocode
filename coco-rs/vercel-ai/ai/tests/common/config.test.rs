use super::*;

#[test]
fn test_env_loading_does_not_panic() {
    ensure_env_loaded();
}

#[test]
fn test_load_test_config_returns_none_for_unconfigured() {
    ensure_env_loaded();
    let cfg = load_test_config("nonexistent_provider_xyz");
    assert!(cfg.is_none());
}
