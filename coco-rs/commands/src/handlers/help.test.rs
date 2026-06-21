use super::*;

#[tokio::test]
async fn test_help_no_args() {
    let result = handler("".to_string()).await.unwrap();
    assert!(result.contains("Available Commands"));
    assert!(result.contains("/help"));
    assert!(result.contains("/config"));
    assert!(result.contains("/diff"));
}

#[tokio::test]
async fn test_help_specific_command() {
    let result = handler("model".to_string()).await.unwrap();
    assert!(result.contains("model"));
}

#[tokio::test]
async fn test_help_unknown_command() {
    let result = handler("nonexistent".to_string()).await.unwrap();
    assert!(result.contains("No command found"));
}

#[tokio::test]
async fn test_help_omits_unregistered_phantom_commands() {
    // These commands are NOT registered in implementations.rs, so the help
    // listing must not advertise them (they would 404 if a user tried them).
    let result = handler("".to_string()).await.unwrap();
    for phantom in ["/fast", "/privacy-settings", "/feedback", "/pr "] {
        assert!(
            !result.contains(phantom),
            "help should not list unregistered command {phantom}"
        );
    }
}

#[tokio::test]
async fn test_help_resume_has_no_continue_alias() {
    // implementations.rs registers /resume with no /continue alias; the
    // static help metadata must match.
    let entry = find_command("resume").expect("resume entry exists");
    assert!(
        !entry.aliases.contains(&"continue"),
        "/resume must not advertise a /continue alias"
    );
}

#[tokio::test]
async fn test_help_clear_and_config_aliases_match_registry() {
    let clear = find_command("clear").expect("clear entry exists");
    assert!(clear.aliases.contains(&"reset") && clear.aliases.contains(&"new"));
    let config = find_command("config").expect("config entry exists");
    assert_eq!(config.aliases, &["settings"]);
    let status = find_command("status").expect("status entry exists");
    assert!(status.aliases.is_empty());
    let tasks = find_command("tasks").expect("tasks entry exists");
    assert_eq!(tasks.aliases, &["bashes"]);
}
