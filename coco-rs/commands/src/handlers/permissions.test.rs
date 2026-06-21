use super::*;

#[tokio::test]
async fn test_list_permissions_output() {
    let output = handler(String::new()).await.unwrap();
    assert!(output.contains("Permission Rules"));
    assert!(output.contains("Session"));
    assert!(output.contains("Project"));
    assert!(output.contains("User"));
    assert!(output.contains("highest to lowest priority"));
}

#[tokio::test]
async fn test_allow_known_tool_non_tui_hint() {
    // Non-TUI handler returns a hint pointing at settings.json. The TUI
    // dispatcher (`tui_runner::dispatch_permissions_mutation`) intercepts
    // this branch and actually mutates engine_config.allow_rules.
    let output = handler("allow Bash".to_string()).await.unwrap();
    assert!(output.contains("/permissions allow Bash"));
    assert!(output.contains("only effective inside the TUI"));
    assert!(output.contains("settings.json"));
    assert!(!output.contains("Warning"));
}

#[tokio::test]
async fn test_allow_unknown_tool_warns() {
    let output = handler("allow CustomTool".to_string()).await.unwrap();
    assert!(output.contains("/permissions allow CustomTool"));
    assert!(output.contains("Warning"));
    assert!(output.contains("not a known built-in tool"));
}

#[tokio::test]
async fn test_deny_tool_non_tui_hint() {
    let output = handler("deny Write".to_string()).await.unwrap();
    assert!(output.contains("/permissions deny Write"));
    assert!(output.contains("only effective inside the TUI"));
}

#[tokio::test]
async fn test_reset_permissions_non_tui_honest() {
    // The handler is honest about not actually mutating state — the TUI
    // dispatcher does the real work in `dispatch_permissions_mutation`.
    let output = handler("reset".to_string()).await.unwrap();
    assert!(output.contains("only effective inside the TUI"));
    assert!(output.contains("File-based rules"));
}

#[tokio::test]
async fn test_unknown_subcommand() {
    let output = handler("foobar".to_string()).await.unwrap();
    assert!(output.contains("Unknown permissions subcommand"));
    assert!(output.contains("Usage"));
}

#[tokio::test]
async fn test_read_permission_rules_from_json() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let settings = serde_json::json!({
        "permissions": {
            "allow": ["Bash", "Read"],
            "deny": ["Write"]
        },
        "allowedTools": ["Glob", "Grep"],
    });
    tokio::fs::write(&settings_path, serde_json::to_string(&settings).unwrap())
        .await
        .unwrap();

    let rules = read_permission_rules_from_path(&settings_path).await;
    assert!(!rules.is_empty());
    // Should contain entries from both "permissions" and "allowedTools"
    assert!(rules.iter().any(|r| r.contains("Bash")));
    assert!(rules.iter().any(|r| r.contains("Glob")));
}

#[tokio::test]
async fn test_read_permission_rules_nonexistent() {
    let rules = read_permission_rules_from_path(Path::new("/tmp/nonexistent_perms.json")).await;
    assert!(rules.is_empty());
}

#[tokio::test]
async fn test_allow_mcp_tool_no_warning() {
    let output = handler("allow mcp__myserver__tool".to_string())
        .await
        .unwrap();
    assert!(output.contains("/permissions allow mcp__myserver__tool"));
    assert!(
        !output.contains("Warning"),
        "MCP tools should be recognized as valid"
    );
}
