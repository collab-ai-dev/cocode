use std::collections::HashMap;

use coco_types::HookEventType;
use coco_types::HookScope;

use super::*;

#[test]
fn test_hook_registry_register_and_find() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Bash".into()),
        handler: HookHandler::Command {
            command: "echo pre-hook".into(),
            timeout_ms: Some(5000),
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    registry.register(HookDefinition {
        event: HookEventType::PostToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo post-hook".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    assert_eq!(registry.len(), 2);

    // Find hooks for PreToolUse on "Bash"
    let matches = registry.find(HookEventType::PreToolUse, Some("Bash"));
    assert_eq!(matches.len(), 1);

    // Find hooks for PreToolUse on "Read" — no matcher match
    let matches = registry.find(HookEventType::PreToolUse, Some("Read"));
    assert_eq!(matches.len(), 0);

    // PostToolUse with no matcher matches everything
    let matches = registry.find(HookEventType::PostToolUse, Some("Bash"));
    assert_eq!(matches.len(), 1);
}

#[test]
fn test_hook_wildcard_matcher() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("*".into()),
        handler: HookHandler::Command {
            command: "echo any".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let matches = registry.find(HookEventType::PreToolUse, Some("anything"));
    assert_eq!(matches.len(), 1);
}

#[test]
fn test_hooks_settings_deserialization() {
    let event_key = HookEventType::PreToolUse.as_str();
    let json_value = serde_json::json!({
        "hooks": {
            event_key: [{
                "event": event_key,
                "matcher": "Bash",
                "handler": {"type": "command", "command": "echo hi"},
            }]
        }
    });
    let json = serde_json::to_string(&json_value).unwrap();
    let settings: HooksSettings = serde_json::from_str(&json).unwrap();
    assert!(settings.hooks.contains_key(event_key));
}

#[test]
fn test_find_matching_returns_sorted_by_scope_then_priority() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "low priority user".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 10,
        scope: HookScope::User,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "high priority user".into(),
            model: None,
            timeout_ms: None,
        },
        priority: -5,
        scope: HookScope::User,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "session scope".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 100,
        scope: HookScope::Session,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let matches = registry.find_matching(HookEventType::PreToolUse, Some("Bash"));
    assert_eq!(matches.len(), 3);
    // Session scope first despite highest priority value
    assert_eq!(matches[0].scope, HookScope::Session);
    // Then User scope sorted by priority ascending
    assert_eq!(matches[1].priority, -5);
    assert_eq!(matches[2].priority, 10);
}

#[test]
fn test_glob_matcher() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Read*".into()),
        handler: HookHandler::Prompt {
            prompt: "matched".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    // Matches glob pattern
    let matches = registry.find_matching(HookEventType::PreToolUse, Some("ReadFile"));
    assert_eq!(matches.len(), 1);

    // Does not match
    let matches = registry.find_matching(HookEventType::PreToolUse, Some("Write"));
    assert_eq!(matches.len(), 0);
}

#[test]
fn test_pipe_separated_matcher() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Write|Edit|Bash".into()),
        handler: HookHandler::Prompt {
            prompt: "matched".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Write"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Edit"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Bash"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Read"))
            .len(),
        0
    );
}

#[test]
fn test_regex_matcher() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("^(Write|Edit)$".into()),
        handler: HookHandler::Prompt {
            prompt: "matched".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Write"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Edit"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("WriteFile"))
            .len(),
        0
    );
}

#[test]
fn test_regex_prefix_matcher() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("^Read.*".into()),
        handler: HookHandler::Prompt {
            prompt: "matched".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Read"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("ReadFile"))
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Write"))
            .len(),
        0
    );
}

#[test]
fn test_prompt_handler() {
    let handler = HookHandler::Prompt {
        prompt: "Check for security issues".into(),
        model: None,
        timeout_ms: None,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = rt
        .block_on(execute_hook(&handler, &HashMap::new(), None))
        .unwrap();

    match result {
        HookExecutionResult::PromptText(text) => {
            assert_eq!(text, "Check for security issues");
        }
        HookExecutionResult::CommandOutput { .. } => {
            panic!("expected PromptText, got CommandOutput");
        }
        HookExecutionResult::SdkOutput(_) => panic!("expected non-SDK variant"),
    }
}

#[tokio::test]
async fn test_command_hook_execution() {
    let handler = HookHandler::Command {
        command: "echo hello-hook".into(),
        timeout_ms: Some(5000),
        shell: None,
    };

    let result = execute_hook(&handler, &HashMap::new(), None).await.unwrap();

    match result {
        HookExecutionResult::CommandOutput {
            exit_code,
            stdout,
            stderr: _,
        } => {
            assert_eq!(exit_code, 0);
            assert_eq!(stdout.trim(), "hello-hook");
        }
        HookExecutionResult::PromptText(_) => {
            panic!("expected CommandOutput, got PromptText");
        }
        HookExecutionResult::SdkOutput(_) => panic!("expected non-SDK variant"),
    }
}

#[tokio::test]
async fn test_execute_hooks_runs_all_matching() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo first".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 1,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "second".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 2,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    // Different event — should not match
    registry.register(HookDefinition {
        event: HookEventType::PostToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "wrong event".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let results = registry
        .execute_hooks(HookEventType::PreToolUse, Some("Bash"))
        .await;
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(results[1].is_ok());
}

#[test]
fn test_no_tool_name_with_wildcard_does_not_match() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("*".into()),
        handler: HookHandler::Prompt {
            prompt: "x".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    // Wildcard requires a tool name to be present
    let matches = registry.find_matching(HookEventType::PreToolUse, None);
    assert_eq!(matches.len(), 0);
}

#[test]
fn test_no_matcher_matches_without_tool_name() {
    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::SessionStart,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo start".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let matches = registry.find_matching(HookEventType::SessionStart, None);
    assert_eq!(matches.len(), 1);
}

#[test]
fn test_scope_ordering() {
    assert!(HookScope::Session > HookScope::Local);
    assert!(HookScope::Local > HookScope::Project);
    assert!(HookScope::Project > HookScope::User);
    assert!(HookScope::User > HookScope::Builtin);
}

#[test]
fn test_all_hook_event_variants_are_present() {
    // All 27 HookEventType names must round-trip through serde.
    // If a new variant is added, this test fails until the new
    // variant is wired in.
    let ts_events = [
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "SessionStart",
        "SessionEnd",
        "Setup",
        "Stop",
        "StopFailure",
        "SubagentStart",
        "SubagentStop",
        "UserPromptSubmit",
        "PermissionRequest",
        "PermissionDenied",
        "Notification",
        "Elicitation",
        "ElicitationResult",
        "PreCompact",
        "PostCompact",
        "TeammateIdle",
        "TaskCreated",
        "TaskCompleted",
        "ConfigChange",
        "InstructionsLoaded",
        "CwdChanged",
        "FileChanged",
        "WorktreeCreate",
        "WorktreeRemove",
    ];
    for name in ts_events {
        let parsed: HookEventType =
            serde_json::from_value(serde_json::Value::String(name.to_string()))
                .unwrap_or_else(|e| panic!("missing event variant {name}: {e}"));
        let registry = HookRegistry::new();
        registry.register(HookDefinition {
            event: parsed,
            matcher: None,
            handler: HookHandler::Prompt {
                prompt: "test".into(),
                model: None,
                timeout_ms: None,
            },
            priority: 0,
            scope: HookScope::default(),
            if_condition: None,
            once: false,
            is_async: false,
            async_rewake: false,
            status_message: None,
        });
        assert_eq!(registry.find_matching(parsed, None).len(), 1);
    }
}

#[test]
fn test_if_condition_filters_matching_hooks() {
    let registry = HookRegistry::new();
    // Hook with if_condition: only matches "Bash(git *)"
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "git-only".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: Some("Bash(git *)".into()),
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });
    // Hook without if_condition: matches everything
    registry.register(HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "any".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 1,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let ctx = IfConditionContext {
        tool_name: "Bash".to_string(),
        tool_content: Some("git status".to_string()),
    };

    // With if_condition context: both match (git hook matches "git status")
    let matches =
        registry.find_matching_with_if(HookEventType::PreToolUse, Some("Bash"), Some(&ctx));
    assert_eq!(matches.len(), 2);

    // Different content: only the unconditional hook matches
    let ctx2 = IfConditionContext {
        tool_name: "Bash".to_string(),
        tool_content: Some("npm install".to_string()),
    };
    let matches =
        registry.find_matching_with_if(HookEventType::PreToolUse, Some("Bash"), Some(&ctx2));
    assert_eq!(matches.len(), 1);

    // Without if_condition context: all hooks returned (no filtering)
    let matches = registry.find_matching_with_if(HookEventType::PreToolUse, Some("Bash"), None);
    assert_eq!(matches.len(), 2);
}

// -----------------------------------------------------------------------
// load_hooks_from_config tests
// -----------------------------------------------------------------------

#[test]
fn test_load_hooks_from_config_command() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo hi",
            "matcher": "Bash"
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::Project).unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, HookEventType::PreToolUse);
    assert_eq!(hooks[0].matcher.as_deref(), Some("Bash"));
    assert_eq!(hooks[0].scope, HookScope::Project);
    match &hooks[0].handler {
        HookHandler::Command { command, shell, .. } => {
            assert_eq!(command, "echo hi");
            assert!(shell.is_none());
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_prompt() {
    let json = serde_json::json!({
        HookEventType::SessionStart.as_str(): [{
            "type": "prompt",
            "prompt": "hello world"
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, HookEventType::SessionStart);
    assert!(hooks[0].matcher.is_none());
    match &hooks[0].handler {
        HookHandler::Prompt { prompt, .. } => assert_eq!(prompt, "hello world"),
        other => panic!("expected Prompt, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_http() {
    let json = serde_json::json!({
        HookEventType::PostToolUse.as_str(): [{
            "type": "webhook",
            "url": "https://example.com/hook",
            "headers": {"Authorization": "Bearer abc"}
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::Local).unwrap();
    assert_eq!(hooks.len(), 1);
    match &hooks[0].handler {
        HookHandler::Http { url, headers, .. } => {
            assert_eq!(url, "https://example.com/hook");
            // The HTTP handler has no `method` field — POST is hardcoded.
            let hdrs = headers.as_ref().unwrap();
            assert_eq!(hdrs.get("Authorization").unwrap(), "Bearer abc");
        }
        other => panic!("expected Http, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_http_type_alias() {
    let json = serde_json::json!({
        HookEventType::PostToolUse.as_str(): [{
            "type": "http",
            "url": "https://example.com/hook2"
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert_eq!(hooks.len(), 1);
    match &hooks[0].handler {
        HookHandler::Http { url, .. } => assert_eq!(url, "https://example.com/hook2"),
        other => panic!("expected Http, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_agent() {
    // Agent hooks have only `prompt` + optional `model`.
    // No `agent_name` field exists.
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "agent",
            "prompt": "review this",
            "model": "claude-sonnet-4-6"
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::Session).unwrap();
    assert_eq!(hooks.len(), 1);
    match &hooks[0].handler {
        HookHandler::Agent { prompt, model, .. } => {
            assert_eq!(prompt, "review this");
            assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        }
        other => panic!("expected Agent, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_with_if_condition() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo check",
            "if": "Bash(git *)"
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert_eq!(hooks[0].if_condition.as_deref(), Some("Bash(git *)"));
}

#[test]
fn test_load_hooks_from_config_with_timeout_seconds() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo slow",
            "timeout": 30
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    match &hooks[0].handler {
        HookHandler::Command { timeout_ms, .. } => {
            assert_eq!(*timeout_ms, Some(30_000));
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_timeout_does_not_override_handler_timeout() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo fast",
            "timeout_ms": 5000,
            "timeout": 60
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    match &hooks[0].handler {
        HookHandler::Command { timeout_ms, .. } => {
            // Handler-level timeout_ms (5000) should be preserved, not overridden
            assert_eq!(*timeout_ms, Some(5000));
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn test_load_hooks_from_config_with_optional_fields() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo hi",
            "once": true,
            "async": true,
            "async_rewake": true,
            "shell": "bash",
            "status_message": "Running check..."
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert!(hooks[0].once);
    assert!(hooks[0].is_async);
    assert!(hooks[0].async_rewake);
    match &hooks[0].handler {
        HookHandler::Command { shell, .. } => {
            assert_eq!(shell.as_deref(), Some("bash"));
        }
        other => panic!("expected Command, got {other:?}"),
    }
    assert_eq!(hooks[0].status_message.as_deref(), Some("Running check..."));
}

#[test]
fn test_loader_policy_disable_all_hooks_drops_everything() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{ "type": "command", "command": "echo hi" }]
    });
    let hooks = load_hooks_from_config_with_policy(
        &json,
        HookScope::Policy,
        LoaderPolicy {
            disable_all_hooks: true,
            allow_managed_hooks_only: false,
        },
    )
    .unwrap();
    assert!(hooks.is_empty(), "disable_all_hooks must drop every entry");
}

#[test]
fn test_loader_policy_allow_managed_hooks_only_drops_user_scope() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{ "type": "command", "command": "echo hi" }]
    });
    // User-scope load should be skipped when managed-only is set.
    let dropped = load_hooks_from_config_with_policy(
        &json,
        HookScope::User,
        LoaderPolicy {
            disable_all_hooks: false,
            allow_managed_hooks_only: true,
        },
    )
    .unwrap();
    assert!(dropped.is_empty());

    // Policy and Session scopes pass through under managed-only.
    let kept_policy = load_hooks_from_config_with_policy(
        &json,
        HookScope::Policy,
        LoaderPolicy {
            disable_all_hooks: false,
            allow_managed_hooks_only: true,
        },
    )
    .unwrap();
    assert_eq!(kept_policy.len(), 1);

    let kept_session = load_hooks_from_config_with_policy(
        &json,
        HookScope::Session,
        LoaderPolicy {
            disable_all_hooks: false,
            allow_managed_hooks_only: true,
        },
    )
    .unwrap();
    assert_eq!(kept_session.len(), 1);
}

#[test]
fn test_load_hooks_from_config_matcher_with_tool_name_object() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{
            "type": "command",
            "command": "echo hi",
            "matcher": {"tool_name": "Write"}
        }]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert_eq!(hooks[0].matcher.as_deref(), Some("Write"));
}

#[test]
fn test_load_hooks_from_config_multiple_events() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [
            {"type": "command", "command": "echo a"},
            {"type": "command", "command": "echo b"}
        ],
        HookEventType::SessionStart.as_str(): [
            {"type": "prompt", "prompt": "welcome"}
        ]
    });

    let hooks = load_hooks_from_config(&json, HookScope::User).unwrap();
    assert_eq!(hooks.len(), 3);
}

#[test]
fn test_load_hooks_from_config_unknown_event_type_errors() {
    let json = serde_json::json!({
        "bogus_event": [{"type": "command", "command": "echo x"}]
    });

    let result = load_hooks_from_config(&json, HookScope::User);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("bogus_event"));
}

#[test]
fn test_load_hooks_from_config_unknown_handler_type_errors() {
    let json = serde_json::json!({
        HookEventType::PreToolUse.as_str(): [{"type": "ftp", "url": "ftp://x"}]
    });

    let result = load_hooks_from_config(&json, HookScope::User);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ftp"));
}

#[test]
fn test_load_hooks_from_config_not_object_errors() {
    let json = serde_json::json!("not an object");
    let result = load_hooks_from_config(&json, HookScope::User);
    assert!(result.is_err());
}

// -----------------------------------------------------------------------
// register_deduped tests
// -----------------------------------------------------------------------

#[test]
fn test_register_deduped_allows_unique_hooks() {
    let registry = HookRegistry::new();
    let h1 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo a".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };
    let h2 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo b".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };

    assert!(registry.register_deduped(h1));
    assert!(registry.register_deduped(h2));
    assert_eq!(registry.len(), 2);
}

#[test]
fn test_register_deduped_rejects_duplicate_command() {
    let registry = HookRegistry::new();
    let h1 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo same".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };
    let h2 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Bash".into()),
        handler: HookHandler::Command {
            command: "echo same".into(),
            timeout_ms: Some(5000),
            shell: None,
        },
        priority: 10,
        scope: HookScope::Session,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };

    assert!(registry.register_deduped(h1));
    // Same command + same shell + same if_condition = duplicate
    assert!(!registry.register_deduped(h2));
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_register_deduped_different_if_condition_not_duplicate() {
    let registry = HookRegistry::new();
    let h1 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo check".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };
    let h2 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo check".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: Some("Bash(git *)".into()),
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };

    assert!(registry.register_deduped(h1));
    assert!(registry.register_deduped(h2));
    assert_eq!(registry.len(), 2);
}

#[test]
fn test_register_deduped_different_shell_not_duplicate() {
    let registry = HookRegistry::new();
    let h1 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo check".into(),
            timeout_ms: None,
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };
    let h2 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo check".into(),
            timeout_ms: None,
            shell: Some("bash".into()),
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };

    assert!(registry.register_deduped(h1));
    assert!(registry.register_deduped(h2));
    assert_eq!(registry.len(), 2);
}

#[test]
fn test_register_deduped_prompt_hooks() {
    let registry = HookRegistry::new();
    let h1 = HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Prompt {
            prompt: "same prompt".into(),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    };
    let h2 = h1.clone();

    assert!(registry.register_deduped(h1));
    assert!(!registry.register_deduped(h2));
    assert_eq!(registry.len(), 1);
}

#[tokio::test]
async fn test_command_hook_custom_shell() {
    let handler = HookHandler::Command {
        command: "echo custom-shell".into(),
        timeout_ms: Some(5000),
        shell: Some("bash".into()),
    };

    let result = execute_hook(&handler, &HashMap::new(), None).await.unwrap();

    match result {
        HookExecutionResult::CommandOutput {
            exit_code, stdout, ..
        } => {
            assert_eq!(exit_code, 0);
            assert_eq!(stdout.trim(), "custom-shell");
        }
        HookExecutionResult::PromptText(_) => {
            panic!("expected CommandOutput, got PromptText");
        }
        HookExecutionResult::SdkOutput(_) => panic!("expected non-SDK variant"),
    }
}

// -----------------------------------------------------------------------
// matcher_matches
// -----------------------------------------------------------------------

#[test]
fn test_matcher_simple_alphanumeric_only() {
    // Pattern with only alphanumeric + underscore → exact match
    assert!(matcher_matches(Some("Read"), Some("Read")));
    assert!(!matcher_matches(Some("Read"), Some("Write")));
}

#[test]
fn test_matcher_regex_with_dot_star() {
    // "Read.*" contains '.' which is not simple → treated as regex
    assert!(matcher_matches(Some("Read.*"), Some("ReadFile")));
    assert!(matcher_matches(Some("Read.*"), Some("Read")));
    assert!(!matcher_matches(Some("Read.*"), Some("Write")));
}

#[test]
fn test_matcher_regex_dot_plus() {
    // "Read.+" is regex, not glob
    assert!(matcher_matches(Some("Read.+"), Some("ReadFile")));
    assert!(!matcher_matches(Some("Read.+"), Some("Read"))); // .+ needs at least one char
}

#[test]
fn test_matcher_pipe_with_special_chars_is_regex() {
    // "Write|Edit|Read.*" has '.' → treated as regex, not pipe-separated
    assert!(matcher_matches(Some("Write|Edit|Read.*"), Some("Write")));
    assert!(matcher_matches(Some("Write|Edit|Read.*"), Some("Edit")));
    assert!(matcher_matches(Some("Write|Edit|Read.*"), Some("ReadFile")));
}

#[test]
fn test_matcher_glob_fallback_on_invalid_regex() {
    // Invalid regex like "[" falls through to glob
    // "[" is also invalid glob, so should return false
    assert!(!matcher_matches(Some("["), Some("anything")));
}

// -----------------------------------------------------------------------
// sanitize_header_value tests
// -----------------------------------------------------------------------

#[test]
fn test_sanitize_header_value_strips_crlf() {
    assert_eq!(sanitize_header_value("normal"), "normal");
    assert_eq!(sanitize_header_value("a\r\nb"), "ab");
    assert_eq!(sanitize_header_value("a\0b"), "ab");
    assert_eq!(
        sanitize_header_value("token\r\nX-Evil: 1"),
        "tokenX-Evil: 1"
    );
}

// -----------------------------------------------------------------------
// interpolate_env_vars_allowlisted tests
// -----------------------------------------------------------------------

#[test]
fn test_interpolate_env_vars_allowlisted_from_hook_env() {
    let mut env = HashMap::new();
    env.insert("MY_TOKEN".to_string(), "secret123".to_string());
    let allow: HashSet<&str> = ["MY_TOKEN"].into_iter().collect();

    assert_eq!(
        interpolate_env_vars_allowlisted("Bearer $MY_TOKEN", &allow, &env),
        "Bearer secret123"
    );
    assert_eq!(
        interpolate_env_vars_allowlisted("Bearer ${MY_TOKEN}", &allow, &env),
        "Bearer secret123"
    );
}

#[test]
fn test_interpolate_env_vars_allowlisted_blocks_unallowed() {
    // Var present in env but NOT in allowlist resolves to "" — prevents
    // exfiltration of arbitrary process env via project hooks.
    let mut env = HashMap::new();
    env.insert("AWS_SECRET_ACCESS_KEY".to_string(), "shhh".to_string());
    let allow: HashSet<&str> = HashSet::new();
    assert_eq!(
        interpolate_env_vars_allowlisted("Bearer $AWS_SECRET_ACCESS_KEY", &allow, &env),
        "Bearer "
    );
}

#[test]
fn test_interpolate_env_vars_allowlisted_missing_resolves_empty() {
    let env = HashMap::new();
    let allow: HashSet<&str> = ["NONEXISTENT_VAR_XYZ"].into_iter().collect();
    assert_eq!(
        interpolate_env_vars_allowlisted("Bearer $NONEXISTENT_VAR_XYZ", &allow, &env),
        "Bearer "
    );
}

#[test]
fn test_interpolate_env_vars_allowlisted_no_vars() {
    let env = HashMap::new();
    let allow: HashSet<&str> = HashSet::new();
    assert_eq!(
        interpolate_env_vars_allowlisted("no vars here", &allow, &env),
        "no vars here"
    );
}

// -----------------------------------------------------------------------
// prompt request types tests
// -----------------------------------------------------------------------

#[test]
fn test_prompt_request_deserialize() {
    let json = r#"{"prompt": "req-1", "message": "Pick one:", "options": [{"key": "a", "label": "Option A"}, {"key": "b", "label": "Option B", "description": "Second option"}]}"#;
    let req: PromptRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.prompt, "req-1");
    assert_eq!(req.options.len(), 2);
    assert_eq!(req.options[1].description.as_deref(), Some("Second option"));
}

#[test]
fn test_prompt_response_serialize() {
    let resp = PromptResponse {
        prompt_response: "req-1".to_string(),
        selected: "a".to_string(),
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("prompt_response"));
    assert!(json.contains("\"a\""));
}

#[test]
fn test_reload_from_runtime_replaces_hooks() {
    let registry = HookRegistry::new();

    let initial = serde_json::json!({
        "PreToolUse": [{
            "matcher": "Bash",
            "type": "command",
            "command": "echo first"
        }]
    });
    registry
        .reload_from_runtime(&[(HookScope::User, initial)], LoaderPolicy::default())
        .expect("reload");
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry.find_matching(HookEventType::PreToolUse, Some("Bash"))[0].matcher,
        Some("Bash".into())
    );

    // Reload with a different hook — old one should be gone.
    let replacement = serde_json::json!({
        "PostToolUse": [{
            "matcher": "Edit",
            "type": "command",
            "command": "echo replaced"
        }]
    });
    let count = registry
        .reload_from_runtime(&[(HookScope::User, replacement)], LoaderPolicy::default())
        .expect("reload");
    assert_eq!(count, 1);
    assert!(
        registry
            .find_matching(HookEventType::PreToolUse, Some("Bash"))
            .is_empty(),
        "old PreToolUse hook should be gone after reload"
    );
    assert_eq!(
        registry.find_matching(HookEventType::PostToolUse, Some("Edit"))[0].matcher,
        Some("Edit".into())
    );
}

#[test]
fn test_reload_preserves_fired_once() {
    let registry = HookRegistry::new();

    let value = serde_json::json!({
        "PreToolUse": [{
            "matcher": "Bash",
            "type": "command",
            "command": "echo once",
            "once": true
        }]
    });
    registry
        .reload_from_runtime(&[(HookScope::User, value.clone())], LoaderPolicy::default())
        .expect("reload");

    // Fire the once hook.
    let matched = registry.find_matching(HookEventType::PreToolUse, Some("Bash"));
    assert_eq!(matched.len(), 1);
    registry.mark_once_fired(&matched[0]);

    // After reload, fired_once should be retained — same hook config
    // shouldn't re-match.
    registry
        .reload_from_runtime(&[(HookScope::User, value)], LoaderPolicy::default())
        .expect("reload");
    let matched_again = registry.find_matching(HookEventType::PreToolUse, Some("Bash"));
    assert!(
        matched_again.is_empty(),
        "fired_once must persist across reload — got {} matches",
        matched_again.len()
    );
}

#[test]
fn test_reload_preserves_agent_scoped() {
    let registry = HookRegistry::new();
    registry.register_for_agent(
        "agent-1".into(),
        vec![HookDefinition {
            event: HookEventType::SubagentStop,
            matcher: None,
            handler: HookHandler::Command {
                command: "echo agent-scoped".into(),
                timeout_ms: None,
                shell: None,
            },
            priority: 0,
            scope: HookScope::default(),
            if_condition: None,
            once: false,
            is_async: false,
            async_rewake: false,
            status_message: None,
        }],
        true,
    );

    // Reload settings — agent_scoped overlay must remain.
    registry
        .reload_from_runtime(
            &[(HookScope::User, serde_json::json!({}))],
            LoaderPolicy::default(),
        )
        .expect("reload");

    assert_eq!(
        registry
            .find_matching(HookEventType::SubagentStop, None)
            .len(),
        1,
        "agent_scoped hooks must survive settings reload"
    );
}

#[test]
fn test_shell_kind_classification() {
    assert_eq!(ShellKind::from_field(None), ShellKind::Bash);
    assert_eq!(ShellKind::from_field(Some("bash")), ShellKind::Bash);
    assert_eq!(ShellKind::from_field(Some("sh")), ShellKind::Bash);
    assert_eq!(
        ShellKind::from_field(Some("powershell")),
        ShellKind::PowerShell
    );
    assert_eq!(ShellKind::from_field(Some("pwsh")), ShellKind::PowerShell);
    // Unknown values fall back to bash with warning.
    assert_eq!(ShellKind::from_field(Some("zsh")), ShellKind::Bash);
}

#[test]
fn test_substitute_plugin_vars_powershell_no_path_xform() {
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_PLUGIN_ROOT".to_string(),
        r"C:\Program Files\Plugin".to_string(),
    );
    let result = substitute_plugin_vars(r"echo ${CLAUDE_PLUGIN_ROOT}", &env, ShellKind::PowerShell);
    // PowerShell consumes native Windows paths — no conversion.
    assert_eq!(result, r"echo C:\Program Files\Plugin");
}

#[cfg(target_os = "windows")]
#[test]
fn test_substitute_plugin_vars_bash_windows_posix_xform() {
    let mut env = HashMap::new();
    env.insert("CLAUDE_PLUGIN_ROOT".to_string(), r"C:\Plugin".to_string());
    let result = substitute_plugin_vars("echo ${CLAUDE_PLUGIN_ROOT}", &env, ShellKind::Bash);
    assert_eq!(result, "echo /c/Plugin");
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_substitute_plugin_vars_bash_non_windows_passthrough() {
    let mut env = HashMap::new();
    env.insert(
        "CLAUDE_PLUGIN_ROOT".to_string(),
        "/home/user/plugin".to_string(),
    );
    let result = substitute_plugin_vars("echo ${CLAUDE_PLUGIN_ROOT}", &env, ShellKind::Bash);
    assert_eq!(result, "echo /home/user/plugin");
}

#[cfg(target_os = "windows")]
#[test]
fn test_maybe_apply_sh_prefix_windows_bash() {
    assert_eq!(
        maybe_apply_sh_prefix("./script.sh", ShellKind::Bash),
        "bash ./script.sh"
    );
    // Already prefixed — passthrough.
    assert_eq!(
        maybe_apply_sh_prefix("bash ./script.sh", ShellKind::Bash),
        "bash ./script.sh"
    );
    // PowerShell — no prefix injection.
    assert_eq!(
        maybe_apply_sh_prefix("./script.sh", ShellKind::PowerShell),
        "./script.sh"
    );
    // Non-.sh extension — passthrough.
    assert_eq!(
        maybe_apply_sh_prefix("./script.py", ShellKind::Bash),
        "./script.py"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_maybe_apply_sh_prefix_non_windows_passthrough() {
    assert_eq!(
        maybe_apply_sh_prefix("./script.sh", ShellKind::Bash),
        "./script.sh"
    );
}

// ── Function hooks ──────────────────────────────────────────────────

#[derive(Debug)]
struct AlwaysPasses;

impl crate::FunctionHookPredicate for AlwaysPasses {
    fn evaluate(&self, _messages: &[std::sync::Arc<coco_messages::Message>]) -> bool {
        true
    }
    fn name(&self) -> &str {
        "AlwaysPasses"
    }
}

#[derive(Debug)]
struct AlwaysFails;

impl crate::FunctionHookPredicate for AlwaysFails {
    fn evaluate(&self, _messages: &[std::sync::Arc<coco_messages::Message>]) -> bool {
        false
    }
    fn name(&self) -> &str {
        "AlwaysFails"
    }
}

#[test]
fn function_hook_register_then_find_returns_match() {
    let registry = HookRegistry::new();
    let id = registry
        .register_function_hook(
            "h-1",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysPasses),
            "must call X",
        )
        .expect("Stop is in FUNCTION_HOOK_SUPPORTED_EVENTS");
    assert_eq!(id, "h-1");
    assert_eq!(registry.function_hook_count(), 1);
    let matches = registry.find_matching_function_hooks(HookEventType::Stop, None);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "h-1");
    assert_eq!(matches[0].predicate.name(), "AlwaysPasses");
}

#[test]
fn function_hook_register_rejects_unsupported_event() {
    let registry = HookRegistry::new();
    // SubagentStop is NOT in FUNCTION_HOOK_SUPPORTED_EVENTS — register
    // must refuse so the hook doesn't persist as a silent no-op.
    let err = registry
        .register_function_hook(
            "h-bad",
            HookEventType::SubagentStop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysPasses),
            "msg",
        )
        .expect_err("non-Stop registration must error");
    assert_eq!(
        err,
        crate::RegisterFunctionHookError::UnsupportedEvent(HookEventType::SubagentStop)
    );
    assert_eq!(
        registry.function_hook_count(),
        0,
        "rejected registration must not leak a hook into storage"
    );
}

#[test]
fn function_hook_register_rejects_duplicate_id() {
    let registry = HookRegistry::new();
    registry
        .register_function_hook(
            "dup",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysPasses),
            "first",
        )
        .unwrap();
    let err = registry
        .register_function_hook(
            "dup",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysFails),
            "second",
        )
        .expect_err("duplicate id must error");
    assert_eq!(
        err,
        crate::RegisterFunctionHookError::DuplicateId("dup".to_string())
    );
    assert_eq!(registry.function_hook_count(), 1);
    // First registration's predicate is preserved.
    let matches = registry.find_matching_function_hooks(HookEventType::Stop, None);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].predicate.name(), "AlwaysPasses");
}

#[test]
fn function_hook_remove_by_id_drops_only_that_hook() {
    let registry = HookRegistry::new();
    registry
        .register_function_hook(
            "h-keep",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysPasses),
            "keep",
        )
        .unwrap();
    registry
        .register_function_hook(
            "h-drop",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysFails),
            "drop",
        )
        .unwrap();
    assert_eq!(registry.function_hook_count(), 2);
    assert!(registry.remove_function_hook("h-drop"));
    assert_eq!(registry.function_hook_count(), 1);
    let matches = registry.find_matching_function_hooks(HookEventType::Stop, None);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "h-keep");
    // Removing an unknown id is a no-op (returns false).
    assert!(!registry.remove_function_hook("h-nonexistent"));
    assert_eq!(registry.function_hook_count(), 1);
}

#[test]
fn function_hook_matcher_none_matches_any_value() {
    let registry = HookRegistry::new();
    registry
        .register_function_hook(
            "h-any",
            HookEventType::Stop,
            None, // wildcard
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(AlwaysPasses),
            "msg",
        )
        .unwrap();
    // Should match any concrete match_value, including None.
    assert_eq!(
        registry
            .find_matching_function_hooks(HookEventType::Stop, None)
            .len(),
        1
    );
    assert_eq!(
        registry
            .find_matching_function_hooks(HookEventType::Stop, Some("any-tool"))
            .len(),
        1
    );
}

#[tokio::test]
async fn test_powershell_hook_returns_helpful_error_when_pwsh_missing() {
    // On a Linux test runner without pwsh installed, a PowerShell hook
    // should return a clear error rather than silently invoking some
    // other shell.
    if coco_shell_discovery::cached_powershell_path()
        .await
        .is_some()
    {
        // skip — pwsh is actually installed; can't assert the missing path
        return;
    }
    let handler = HookHandler::Command {
        command: "Write-Host hi".to_string(),
        timeout_ms: Some(5000),
        shell: Some("powershell".to_string()),
    };
    let env = HashMap::new();
    let result = execute_hook(&handler, &env, None).await;
    let err = result.expect_err("expected error when pwsh is not on PATH");
    let msg = err.to_string();
    assert!(
        msg.contains("PowerShell"),
        "expected error to mention PowerShell, got: {msg}",
    );
}
