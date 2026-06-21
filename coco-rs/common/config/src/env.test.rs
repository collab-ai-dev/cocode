use super::*;

#[test]
fn test_env_key_as_str() {
    assert_eq!(EnvKey::CocoAgentName.as_str(), "COCO_AGENT_NAME");
    assert_eq!(
        EnvKey::CocoMaxToolUseConcurrency.as_str(),
        "COCO_MAX_TOOL_USE_CONCURRENCY"
    );
    assert_eq!(
        EnvKey::CocoMcpToolTimeoutMs.as_str(),
        "COCO_MCP_TOOL_TIMEOUT_MS"
    );
}

#[test]
fn test_std_env_var_accepts_env_key() {
    // SAFETY: tests run single-threaded for env-mutating cases.
    unsafe {
        std::env::set_var(EnvKey::CocoAgentName, "test-agent");
    }
    assert_eq!(
        var(EnvKey::CocoAgentName).ok().as_deref(),
        Some("test-agent")
    );
    unsafe {
        std::env::remove_var(EnvKey::CocoAgentName);
    }
}

#[test]
fn test_is_env_truthy_values() {
    for (val, expected) in [
        ("1", true),
        ("true", true),
        ("TRUE", true),
        ("yes", true),
        ("on", true),
        ("0", false),
        ("false", false),
        ("", false),
        ("anything", false),
    ] {
        // SAFETY: test-only, single-threaded context
        unsafe { std::env::set_var("_COCO_TEST_TRUTHY", val) };
        assert_eq!(
            is_env_truthy("_COCO_TEST_TRUTHY"),
            expected,
            "is_env_truthy({val:?})"
        );
    }
    unsafe { std::env::remove_var("_COCO_TEST_TRUTHY") };
}

#[test]
fn test_is_env_truthy_unset() {
    unsafe { std::env::remove_var("_COCO_TEST_UNSET") };
    assert!(!is_env_truthy("_COCO_TEST_UNSET"));
}

#[test]
fn test_is_env_falsy_values() {
    for (val, expected) in [
        ("0", true),
        ("false", true),
        ("FALSE", true),
        ("no", true),
        ("off", true),
        ("1", false),
        ("true", false),
    ] {
        unsafe { std::env::set_var("_COCO_TEST_FALSY", val) };
        assert_eq!(
            is_env_falsy("_COCO_TEST_FALSY"),
            expected,
            "is_env_falsy({val:?})"
        );
    }
    unsafe { std::env::remove_var("_COCO_TEST_FALSY") };
}

#[test]
fn test_env_snapshot_from_pairs() {
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoMaxToolUseConcurrency, "7"),
        (EnvKey::CocoSimple, "true"),
    ]);

    assert_eq!(env.get_i32(EnvKey::CocoMaxToolUseConcurrency), Some(7));
    assert!(env.is_truthy(EnvKey::CocoSimple));
    assert_eq!(env.get(EnvKey::CocoModelMain), None);
}

#[test]
fn test_env_only_config_from_snapshot() {
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoModelMain, "openai/gpt-5"),
        (EnvKey::CocoSimple, "true"),
    ]);

    let config = EnvOnlyConfig::from_snapshot(&env);

    assert_eq!(config.model_override.as_deref(), Some("openai/gpt-5"));
    assert!(config.force_env_auth);
}
