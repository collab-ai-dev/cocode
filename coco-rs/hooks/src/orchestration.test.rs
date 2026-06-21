use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_types::HookEventType;
use coco_types::HookScope;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::HookDefinition;
use crate::HookHandler;
use crate::HookRegistry;

fn test_ctx() -> OrchestrationContext {
    OrchestrationContext {
        session_id: "test-session-123".to_string(),
        cwd: PathBuf::from("/tmp/test"),
        project_dir: Some(PathBuf::from("/tmp/project")),
        permission_mode: Some("default".to_string()),
        transcript_path: None,
        agent_id: None,
        agent_type: None,
        cancel: CancellationToken::new(),
        disable_all_hooks: false,
        allow_managed_hooks_only: false,
        attachment_emitter: coco_messages::AttachmentEmitter::noop(),
        sync_event_sink: None,
        http_url_allowlist: None,
        http_env_var_policy: None,
        async_registry: None,
        async_rewake_sink: None,
        llm_handle: None,
        workspace_trust_accepted: None,
    }
}

fn make_registry(hooks: Vec<HookDefinition>) -> HookRegistry {
    let registry = HookRegistry::new();
    for h in hooks {
        registry.register(h);
    }
    registry
}

#[tokio::test]
async fn sdk_callback_hook_routes_through_registered_runtime_callback() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Bash".to_string()),
        handler: HookHandler::SdkCallback {
            callback_id: "cb-1".to_string(),
            timeout_ms: None,
        },
        priority: 0,
        scope: HookScope::Session,
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }]);
    let called = Arc::new(AtomicBool::new(false));
    let called_for_callback = called.clone();
    registry.set_sdk_hook_callback(Arc::new(move |request| {
        assert_eq!(request.callback_id, "cb-1");
        assert_eq!(request.event, HookEventType::PreToolUse);
        assert_eq!(request.tool_use_id.as_deref(), Some("tool-1"));
        called_for_callback.store(true, Ordering::SeqCst);
        // Typed SdkHookOutput — no JSON round-trip. PreToolUse deny
        // via hookSpecificOutput is the canonical shape for "block
        // this tool with a reason".
        Box::pin(async {
            Ok(coco_types::SdkHookOutput {
                hook_specific_output: Some(coco_types::HookSpecificOutput::PreToolUse {
                    permission_decision: Some(coco_types::HookPermissionDecision::Deny),
                    permission_decision_reason: Some("sdk denied".into()),
                    updated_input: None,
                    additional_context: None,
                }),
                ..Default::default()
            })
        })
    }));

    let result = execute_pre_tool_use(
        &registry,
        &test_ctx(),
        "Bash",
        "tool-1",
        &serde_json::json!({ "command": "rm -rf /tmp/x" }),
        None,
    )
    .await
    .unwrap();

    assert!(called.load(Ordering::SeqCst));
    let err = result
        .blocking_error
        .as_ref()
        .expect("SDK callback deny should produce blocking_error");
    assert_eq!(err.blocking_error, "sdk denied");
    // Regression guard: SDK callback denials carry `HookBlockingSource::Sdk`
    // — not `Command(label)`. Telemetry and log filtering can distinguish
    // SDK denials from shell-command denials without parsing the label.
    match &err.source {
        HookBlockingSource::Sdk { callback_id } => assert_eq!(callback_id, "cb-1"),
        other => panic!("expected HookBlockingSource::Sdk, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// parse_hook_output tests
// -----------------------------------------------------------------------

#[test]
fn test_parse_hook_output_plain_text() {
    let parsed = parse_hook_output("just some text\n");
    match parsed {
        ParsedHookOutput::PlainText(t) => assert_eq!(t, "just some text\n"),
        other => panic!("expected PlainText, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_output_json_decision() {
    let json_str = r#"{"decision": "block", "reason": "not allowed"}"#;
    let parsed = parse_hook_output(json_str);
    match parsed {
        ParsedHookOutput::Json(j) => {
            assert_eq!(j.decision.as_deref(), Some("block"));
            assert_eq!(j.reason.as_deref(), Some("not allowed"));
        }
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_output_json_continue_false() {
    let json_str = r#"{"continue": false, "stop_reason": "user abort"}"#;
    let parsed = parse_hook_output(json_str);
    match parsed {
        ParsedHookOutput::Json(j) => {
            assert_eq!(j.should_continue, Some(false));
            assert_eq!(j.stop_reason.as_deref(), Some("user abort"));
        }
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_output_invalid_json_falls_back_to_plain() {
    let bad = r#"{ invalid json }"#;
    let parsed = parse_hook_output(bad);
    match parsed {
        ParsedHookOutput::PlainText(t) => assert_eq!(t, bad),
        other => panic!("expected PlainText for invalid JSON, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_output_valid_json_wrong_shape_is_validation_error() {
    // Valid JSON, but `continue` must be a boolean — schema mismatch surfaces
    // as a ValidationError (not PlainText), so it is never injected as context.
    let parsed = parse_hook_output(r#"{"continue": "yes please"}"#);
    match parsed {
        ParsedHookOutput::ValidationError(msg) => {
            assert!(msg.contains("Expected schema"), "got: {msg}");
        }
        other => panic!("expected ValidationError, got {other:?}"),
    }
}

#[test]
fn test_aggregate_suppresses_validation_error_from_context() {
    // A hook whose stdout is valid-JSON-wrong-shape must not inject its raw
    // text into additional_contexts (validation-error path).
    let result = SingleHookResult {
        command: "h".into(),
        succeeded: true,
        output: r#"{"continue": "yes please"}"#.into(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command("h".into()),
        sdk_output: None,
    };
    let agg = aggregate_results(std::slice::from_ref(&result));
    assert!(
        agg.additional_contexts.is_empty(),
        "validation-error stdout must not be injected as context"
    );
}

// -----------------------------------------------------------------------
// aggregate_results tests
// -----------------------------------------------------------------------

#[test]
fn test_aggregate_results_empty() {
    let agg = aggregate_results(&[]);
    assert!(!agg.is_blocked());
    assert!(!agg.prevent_continuation);
    assert!(agg.additional_contexts.is_empty());
}

#[test]
fn test_aggregate_results_blocking() {
    let results = vec![SingleHookResult {
        command: "check.sh".to_string(),
        succeeded: false,
        output: "forbidden".to_string(),
        blocked: true,
        outcome: HookOutcome::Blocking,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(agg.is_blocked());
    assert_eq!(
        agg.blocking_error
            .as_ref()
            .map(|e| e.blocking_error.as_str()),
        Some("forbidden")
    );
}

#[test]
fn test_aggregate_results_json_permission_deny() {
    let json_output = r#"{"decision": "block", "reason": "security policy"}"#;
    let results = vec![SingleHookResult {
        command: "policy.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(agg.is_blocked());
    assert_eq!(agg.permission_behavior, Some(PermissionBehavior::Deny));
    assert_eq!(
        agg.hook_permission_decision_reason.as_deref(),
        Some("security policy")
    );
}

#[test]
fn test_aggregate_results_json_permission_allow() {
    let json_output = r#"{"decision": "approve", "reason": "pre-approved"}"#;
    let results = vec![SingleHookResult {
        command: "approve.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(!agg.is_blocked());
    assert_eq!(agg.permission_behavior, Some(PermissionBehavior::Allow));
}

#[test]
fn test_aggregate_results_flat_permission_ask() {
    let json_output = r#"{"permissionDecision": "ask", "reason": "needs review"}"#;
    let results = vec![SingleHookResult {
        command: "ask.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(!agg.is_blocked());
    assert_eq!(agg.permission_behavior, Some(PermissionBehavior::Ask));
    assert_eq!(
        agg.hook_permission_decision_reason.as_deref(),
        Some("needs review")
    );
}

#[test]
fn test_aggregate_results_additional_context() {
    let results = vec![
        SingleHookResult {
            command: "ctx1.sh".to_string(),
            succeeded: true,
            output: "context alpha".to_string(),
            blocked: false,
            outcome: HookOutcome::Success,
            status_message: None,
            async_rewake: false,
            source: HookBlockingSource::Command(String::new()),
            sdk_output: None,
        },
        SingleHookResult {
            command: "ctx2.sh".to_string(),
            succeeded: true,
            output: "context beta".to_string(),
            blocked: false,
            outcome: HookOutcome::Success,
            status_message: None,
            async_rewake: false,
            source: HookBlockingSource::Command(String::new()),
            sdk_output: None,
        },
    ];
    let agg = aggregate_results(&results);
    assert_eq!(agg.additional_contexts.len(), 2);
    assert_eq!(agg.additional_contexts[0], "context alpha");
    assert_eq!(agg.additional_contexts[1], "context beta");
}

#[test]
fn test_aggregate_results_failed_plaintext_not_injected() {
    // A failed (non-zero, non-2) hook's stderr is NOT injected as
    // model context — it surfaces only as a hook_non_blocking_error.
    let results = vec![SingleHookResult {
        command: "boom.sh".to_string(),
        succeeded: false,
        output: "boom on stderr".to_string(),
        blocked: false,
        outcome: HookOutcome::NonBlockingError,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(
        agg.additional_contexts.is_empty(),
        "failed-hook stderr must not become model context"
    );
}

// -----------------------------------------------------------------------
// build_hook_env tests
// -----------------------------------------------------------------------

#[test]
fn test_build_hook_env_basic() {
    let env = build_hook_env(
        "sess-1",
        "/home/user",
        Some("Bash"),
        HookEventType::PreToolUse,
        None,
    );
    assert_eq!(env.get("HOOK_EVENT").unwrap(), "PreToolUse");
    assert_eq!(env.get("HOOK_SESSION_ID").unwrap(), "sess-1");
    assert_eq!(env.get("HOOK_TOOL_NAME").unwrap(), "Bash");
    assert!(!env.contains_key("CLAUDE_PROJECT_DIR"));
}

#[test]
fn test_build_hook_env_with_project_dir() {
    let env = build_hook_env(
        "sess-2",
        "/tmp",
        None,
        HookEventType::SessionStart,
        Some("/proj/root"),
    );
    assert!(!env.contains_key("HOOK_TOOL_NAME"));
    assert_eq!(env.get("CLAUDE_PROJECT_DIR").unwrap(), "/proj/root");
}

// -----------------------------------------------------------------------
// execute_hooks_parallel tests (integration with actual shell execution)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_parallel_execution_multiple_hooks() {
    let registry = make_registry(vec![
        HookDefinition {
            event: HookEventType::PreToolUse,
            matcher: None,
            handler: HookHandler::Command {
                command: "echo hook-a".to_string(),
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
        },
        HookDefinition {
            event: HookEventType::PreToolUse,
            matcher: None,
            handler: HookHandler::Command {
                command: "echo hook-b".to_string(),
                timeout_ms: Some(5000),
                shell: None,
            },
            priority: 1,
            scope: HookScope::default(),
            if_condition: None,
            once: false,
            is_async: false,
            async_rewake: false,
            status_message: None,
        },
    ]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::PreToolUse,
        Some("Bash"),
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(10),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(r.succeeded);
        assert!(!r.blocked);
        assert_eq!(r.outcome, HookOutcome::Success);
    }

    // Both should produce output (order may vary since they run in parallel).
    let outputs: Vec<&str> = results.iter().map(|r| r.output.trim()).collect();
    assert!(outputs.contains(&"hook-a"));
    assert!(outputs.contains(&"hook-b"));
}

#[tokio::test]
async fn test_parallel_execution_cancellation() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::SessionEnd,
        matcher: None,
        handler: HookHandler::Command {
            command: "sleep 60".to_string(),
            timeout_ms: Some(60_000),
            shell: None,
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }]);

    let cancel = CancellationToken::new();
    // Cancel immediately.
    cancel.cancel();

    let results = execute_hooks_parallel(
        &registry,
        HookEventType::SessionEnd,
        None,
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(1),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].succeeded);
    assert_eq!(results[0].outcome, HookOutcome::Cancelled);
}

#[tokio::test]
async fn test_parallel_execution_timeout() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::SessionEnd,
        matcher: None,
        handler: HookHandler::Command {
            command: "sleep 60".to_string(),
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
    }]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::SessionEnd,
        None,
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_millis(100),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].succeeded);
    assert!(results[0].output.contains("timed out"));
}

#[tokio::test]
async fn test_parallel_execution_exit_code_2_is_blocking() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo 'blocked' >&2; exit 2".to_string(),
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
    }]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::PreToolUse,
        Some("Write"),
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(5),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].succeeded);
    assert!(results[0].blocked);
    assert!(results[0].output.contains("blocked"));
}

// -----------------------------------------------------------------------
// Event-specific orchestration tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_execute_pre_tool_use_with_prompt_hook() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: Some("Bash".to_string()),
        handler: HookHandler::Prompt {
            prompt: "Check for dangerous commands".to_string(),
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
    }]);

    let ctx = test_ctx();
    let result = execute_pre_tool_use(
        &registry,
        &ctx,
        "Bash",
        "tool-use-1",
        &serde_json::json!({"command": "rm -rf /"}),
        /*event_tx*/ None,
    )
    .await
    .expect("should succeed");

    assert!(!result.is_blocked());
    assert_eq!(result.additional_contexts.len(), 1);
    assert_eq!(
        result.additional_contexts[0],
        "Check for dangerous commands"
    );
}

#[tokio::test]
async fn test_execute_post_tool_use_failure_with_prompt_hook() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::PostToolUseFailure,
        matcher: Some("Bash".to_string()),
        handler: HookHandler::Prompt {
            prompt: "Recover from failed command".to_string(),
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
    }]);

    let ctx = test_ctx();
    let result = execute_post_tool_use_failure(
        &registry,
        &ctx,
        "Bash",
        "tool-use-1",
        &serde_json::json!({"command": "false"}),
        "exit code 1",
        /*is_interrupt*/ None,
        /*event_tx*/ None,
    )
    .await
    .expect("should succeed");

    assert!(!result.is_blocked());
    assert_eq!(result.additional_contexts.len(), 1);
    assert_eq!(result.additional_contexts[0], "Recover from failed command");
}

#[tokio::test]
async fn test_execute_session_start() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::SessionStart,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo initialized".to_string(),
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
    }]);

    let ctx = test_ctx();
    let result = execute_session_start(
        &registry,
        &ctx,
        crate::inputs::SessionStartSource::Startup,
        None,
        None,
    )
    .await
    .expect("should succeed");

    assert!(!result.is_blocked());
    // The "initialized" output appears as additional context (plain text).
    assert!(
        result
            .additional_contexts
            .iter()
            .any(|c| c == "initialized"),
        "expected 'initialized' in contexts: {:?}",
        result.additional_contexts
    );
}

#[tokio::test]
async fn test_execute_session_start_collect_events_does_not_push_sync_buffer() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::SessionStart,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo initialized".to_string(),
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
    }]);

    let sync = crate::SyncHookEventBuffer::new();
    let mut ctx = test_ctx();
    ctx.sync_event_sink = Some(sync.clone());

    let result = execute_session_start_collect_events(
        &registry,
        &ctx,
        crate::inputs::SessionStartSource::Compact,
        None,
        None,
    )
    .await
    .expect("should succeed");

    assert!(
        result.events.iter().any(|event| matches!(
            event,
            coco_system_reminder::HookEvent::Success {
                hook_event: coco_system_reminder::HookEventKind::SessionStart,
                content,
                ..
            } if content.contains("initialized")
        )),
        "expected SessionStart success event: {:?}",
        result.events
    );
    assert!(
        sync.drain().await.is_empty(),
        "collect-events path must not also enqueue next-turn hook reminders"
    );
}

// ── execute_stop + function hooks (integration) ────────────────────

#[derive(Debug)]
struct CountsTextOccurrences {
    needle: String,
    min: usize,
}

impl crate::FunctionHookPredicate for CountsTextOccurrences {
    fn evaluate(&self, messages: &[std::sync::Arc<coco_messages::Message>]) -> bool {
        let count = messages
            .iter()
            .filter(|m| {
                matches!(
                    m.as_ref(),
                    coco_messages::Message::User(u) if matches!(
                        &u.message,
                        coco_messages::LlmMessage::User { content, .. }
                            if content.iter().any(|p| matches!(
                                p,
                                coco_messages::UserContent::Text(t) if t.text.contains(&self.needle)
                            ))
                    )
                )
            })
            .count();
        count >= self.min
    }
    fn name(&self) -> &str {
        "CountsTextOccurrences"
    }
}

#[tokio::test]
async fn execute_stop_fires_function_hook_and_surfaces_blocking_error() {
    // Predicate requires the literal "DONE" to appear in some user
    // message; history has no such marker, so the hook must return
    // false → execute_stop populates `agg.blocking_error` with the
    // hook's error_message and `source = HookBlockingSource::Function`.
    let registry = HookRegistry::new();
    let predicate = std::sync::Arc::new(CountsTextOccurrences {
        needle: "DONE".to_string(),
        min: 1,
    });
    registry
        .register_function_hook(
            "stop-needs-done",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            predicate,
            "must say DONE",
        )
        .unwrap();

    let history = vec![std::sync::Arc::new(make_user_msg("hello"))];
    let agg = execute_stop(&registry, &test_ctx(), false, None, &history, None)
        .await
        .unwrap();

    let err = agg
        .blocking_error
        .as_ref()
        .expect("function hook should block Stop");
    assert_eq!(err.blocking_error, "must say DONE");
    match &err.source {
        crate::orchestration::HookBlockingSource::Function { hook_id } => {
            assert_eq!(hook_id, "stop-needs-done")
        }
        other => panic!("expected Function source, got {other:?}"),
    }
}

#[tokio::test]
async fn execute_stop_function_hook_allows_when_predicate_passes() {
    // Same predicate, but this history contains "DONE" → predicate
    // returns true → execute_stop returns aggregate with NO blocking
    // error. Mirrors the success path of StructuredOutput enforcement
    // after the model finally calls the tool.
    let registry = HookRegistry::new();
    registry
        .register_function_hook(
            "stop-needs-done",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(CountsTextOccurrences {
                needle: "DONE".to_string(),
                min: 1,
            }),
            "must say DONE",
        )
        .unwrap();

    let history = vec![std::sync::Arc::new(make_user_msg("DONE"))];
    let agg = execute_stop(&registry, &test_ctx(), false, None, &history, None)
        .await
        .unwrap();
    assert!(agg.blocking_error.is_none(), "predicate passed; no block");
}

#[tokio::test]
async fn execute_stop_function_hook_settings_takes_precedence_over_function() {
    // When BOTH a settings hook (Command via JSON stdout) AND a
    // function hook block, `apply_function_hook_results` honors
    // first-blocker-wins: settings populates the slot first, so the
    // surfaced source is `Command(...)`, not `Function`.
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::Stop,
        matcher: None,
        // JSON-mode hook that blocks with reason="settings says no"
        handler: HookHandler::Command {
            command: r#"echo '{"continue": false, "stopReason": "settings says no"}'"#.to_string(),
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
    }]);
    registry
        .register_function_hook(
            "function-also-blocks",
            HookEventType::Stop,
            None,
            std::time::Duration::from_secs(1),
            std::sync::Arc::new(CountsTextOccurrences {
                needle: "DONE".to_string(),
                min: 1,
            }),
            "function says no",
        )
        .unwrap();

    let history: Vec<std::sync::Arc<coco_messages::Message>> = Vec::new();
    let agg = execute_stop(&registry, &test_ctx(), false, None, &history, None)
        .await
        .unwrap();

    // Settings hook signals prevent_continuation via JSON; aggregate
    // captures that. blocking_error is populated only when a hook
    // sets `blocked = true`; a `continue: false` without
    // a `decision: block` carries prevent_continuation instead. So we
    // assert: function hook STILL doesn't win the blocking_error slot
    // here — even when settings stays silent on blocking_error, the
    // function hook fills it solo. Verify the source discriminator.
    if let Some(err) = agg.blocking_error.as_ref() {
        match &err.source {
            crate::orchestration::HookBlockingSource::Function { hook_id } => {
                assert_eq!(hook_id, "function-also-blocks");
                assert_eq!(err.blocking_error, "function says no");
            }
            other => panic!("expected Function source for solo block, got {other:?}"),
        }
    }
    // prevent_continuation set by the settings JSON either way:
    assert!(
        agg.prevent_continuation,
        "settings JSON `continue: false` must set prevent_continuation"
    );
}

fn make_user_msg(text: &str) -> coco_messages::Message {
    coco_messages::Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(text.to_string()),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

#[tokio::test]
async fn test_execute_stop_failure() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::StopFailure,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo stop-failure-handled".to_string(),
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
    }]);

    let ctx = test_ctx();
    let results = execute_stop_failure(
        &registry,
        &ctx,
        "context_overflow",
        Some("token limit exceeded"),
        None,
    )
    .await
    .expect("should succeed");

    assert_eq!(results.len(), 1);
    assert!(results[0].succeeded);
    assert_eq!(results[0].output.trim(), "stop-failure-handled");
}

// -----------------------------------------------------------------------
// Phase 3 entry points — smoke tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_execute_setup_routes_match_value_to_trigger() {
    // Setup hooks match on the `trigger` field — verify the hook only
    // fires for the matcher that lines up with the trigger we pass.
    let registry = make_registry(vec![
        HookDefinition {
            event: HookEventType::Setup,
            matcher: Some("init".to_string()),
            handler: HookHandler::Command {
                command: "echo init-only".to_string(),
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
        },
        HookDefinition {
            event: HookEventType::Setup,
            matcher: Some("maintenance".to_string()),
            handler: HookHandler::Command {
                command: "echo maintenance-only".to_string(),
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
        },
    ]);

    let ctx = test_ctx();
    let result = execute_setup(&registry, &ctx, crate::inputs::SetupTrigger::Init)
        .await
        .expect("should succeed");
    assert!(
        result.additional_contexts.iter().any(|c| c == "init-only"),
        "expected init-only fire, got contexts: {:?}",
        result.additional_contexts
    );
    assert!(
        !result
            .additional_contexts
            .iter()
            .any(|c| c == "maintenance-only"),
        "maintenance-only must not fire on init trigger"
    );
}

#[tokio::test]
async fn test_execute_config_change_routes_match_value_to_source() {
    // ConfigChange hooks match on the `source` field — verify the
    // matcher correctly targets the policy_settings source.
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::ConfigChange,
        matcher: Some("policy_settings".to_string()),
        handler: HookHandler::Command {
            command: "echo policy-changed".to_string(),
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
    }]);

    let ctx = test_ctx();
    let fired = execute_config_change(
        &registry,
        &ctx,
        crate::inputs::ConfigChangeSource::PolicySettings,
        None,
    )
    .await
    .expect("should succeed");
    assert!(
        fired
            .additional_contexts
            .iter()
            .any(|c| c == "policy-changed")
    );

    let skipped = execute_config_change(
        &registry,
        &ctx,
        crate::inputs::ConfigChangeSource::UserSettings,
        None,
    )
    .await
    .expect("should succeed");
    assert!(skipped.additional_contexts.is_empty());
}

#[tokio::test]
async fn test_execute_file_changed_matches_basename() {
    // FileChanged matcher gates on basename of file_path. A pipe-
    // separated matcher like ".envrc|.env" matches both files.
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::FileChanged,
        matcher: Some(".envrc|.env".to_string()),
        handler: HookHandler::Command {
            command: "echo env-changed".to_string(),
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
    }]);

    let ctx = test_ctx();
    let fired = execute_file_changed(
        &registry,
        &ctx,
        "/proj/.envrc",
        crate::inputs::FileChangeEvent::Change,
    )
    .await
    .expect("should succeed");
    assert!(
        fired.additional_contexts.iter().any(|c| c == "env-changed"),
        "expected env-changed for /proj/.envrc, got {:?}",
        fired.additional_contexts
    );

    let skipped = execute_file_changed(
        &registry,
        &ctx,
        "/proj/Cargo.toml",
        crate::inputs::FileChangeEvent::Change,
    )
    .await
    .expect("should succeed");
    assert!(skipped.additional_contexts.is_empty());
}

#[tokio::test]
async fn test_execute_task_event_helpers_dispatch_to_distinct_events() {
    // The TaskCreated / TaskCompleted / TeammateIdle helpers each
    // build their own input struct and route through distinct
    // HookEventType variants. A hook registered for one
    // must not fire for the other two.
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::TaskCompleted,
        matcher: None,
        handler: HookHandler::Command {
            command: "echo done".to_string(),
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
    }]);

    let ctx = test_ctx();
    let created = execute_task_created(&registry, &ctx, "task-1", "subject-1", None, None, None)
        .await
        .expect("should succeed");
    assert!(created.additional_contexts.is_empty());

    let completed =
        execute_task_completed(&registry, &ctx, "task-1", "subject-1", None, None, None)
            .await
            .expect("should succeed");
    assert!(completed.additional_contexts.iter().any(|c| c == "done"));

    let idle = execute_teammate_idle(&registry, &ctx, "teammate-1", "team-1")
        .await
        .expect("should succeed");
    assert!(idle.additional_contexts.is_empty());
}

// -----------------------------------------------------------------------
// HTTP hook filtering for SessionStart/Setup
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_http_hooks_filtered_for_session_start() {
    let registry = make_registry(vec![
        HookDefinition {
            event: HookEventType::SessionStart,
            matcher: None,
            handler: HookHandler::Http {
                url: "https://example.com/hook".to_string(),
                headers: None,
                timeout_ms: Some(5000),
                allowed_env_vars: Vec::new(),
            },
            priority: 0,
            scope: HookScope::default(),
            if_condition: None,
            once: false,
            is_async: false,
            async_rewake: false,
            status_message: None,
        },
        HookDefinition {
            event: HookEventType::SessionStart,
            matcher: None,
            handler: HookHandler::Command {
                command: "echo allowed".to_string(),
                timeout_ms: Some(5000),
                shell: None,
            },
            priority: 1,
            scope: HookScope::default(),
            if_condition: None,
            once: false,
            is_async: false,
            async_rewake: false,
            status_message: None,
        },
    ]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::SessionStart,
        None,
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(5),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    // HTTP hook should be filtered out, only command hook remains
    assert_eq!(results.len(), 1);
    assert!(results[0].succeeded);
    assert_eq!(results[0].output.trim(), "allowed");
}

#[tokio::test]
async fn test_http_hooks_filtered_for_setup() {
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::Setup,
        matcher: None,
        handler: HookHandler::Http {
            url: "https://example.com/setup".to_string(),
            headers: None,
            timeout_ms: Some(5000),
            allowed_env_vars: Vec::new(),
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::Setup,
        None,
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(5),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    // All hooks were HTTP, so all filtered out
    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn test_http_hooks_allowed_for_other_events() {
    // HTTP hooks should NOT be filtered for non-SessionStart/Setup events
    let registry = make_registry(vec![HookDefinition {
        event: HookEventType::PreToolUse,
        matcher: None,
        handler: HookHandler::Http {
            url: "https://example.com/pre".to_string(),
            headers: None,
            timeout_ms: Some(5000),
            allowed_env_vars: Vec::new(),
        },
        priority: 0,
        scope: HookScope::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    }]);

    let cancel = CancellationToken::new();
    let results = execute_hooks_parallel(
        &registry,
        HookEventType::PreToolUse,
        Some("Bash"),
        "{}",
        &HashMap::new(),
        &cancel,
        Duration::from_secs(5),
        /*event_tx*/ None,
        &coco_messages::AttachmentEmitter::noop(),
    )
    .await;

    // HTTP hook should NOT be filtered for PreToolUse
    assert_eq!(results.len(), 1);
}

// -----------------------------------------------------------------------
// Formatting helpers
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// hookSpecificOutput parsing tests
// -----------------------------------------------------------------------

#[test]
fn test_parse_hook_specific_output_pre_tool_use() {
    let json_str = r#"{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow", "additionalContext": "safe command"}}"#;
    let parsed = parse_hook_output(json_str);
    match parsed {
        ParsedHookOutput::Json(j) => {
            assert!(j.hook_specific_output.is_some());
            match j.hook_specific_output.as_ref().unwrap() {
                HookSpecificOutput::PreToolUse {
                    permission_decision,
                    additional_context,
                    ..
                } => {
                    assert_eq!(*permission_decision, Some(HookPermissionDecision::Allow));
                    assert_eq!(additional_context.as_deref(), Some("safe command"));
                }
                other => panic!("expected PreToolUse, got {other:?}"),
            }
        }
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_specific_output_permission_request() {
    let json_str = r#"{"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow", "updatedInput": {"command": "ls"}}}}"#;
    let parsed = parse_hook_output(json_str);
    match parsed {
        ParsedHookOutput::Json(j) => match j.hook_specific_output.as_ref().unwrap() {
            HookSpecificOutput::PermissionRequest { decision } => {
                match decision.as_ref().unwrap() {
                    PermissionRequestDecision::Allow { updated_input } => {
                        assert!(updated_input.is_some());
                    }
                    PermissionRequestDecision::Deny { .. } => panic!("expected Allow"),
                }
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        },
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn test_parse_hook_specific_output_elicitation_decline() {
    let json_str =
        r#"{"hookSpecificOutput": {"hookEventName": "Elicitation", "action": "decline"}}"#;
    let parsed = parse_hook_output(json_str);
    match parsed {
        ParsedHookOutput::Json(j) => match j.hook_specific_output.as_ref().unwrap() {
            HookSpecificOutput::Elicitation { action, .. } => {
                assert_eq!(*action, Some(ElicitationAction::Decline));
            }
            other => panic!("expected Elicitation, got {other:?}"),
        },
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn test_aggregate_with_hook_specific_output() {
    let json_output = r#"{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "dangerous", "additionalContext": "blocked cmd"}}"#;
    let results = vec![SingleHookResult {
        command: "check.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert_eq!(agg.permission_behavior, Some(PermissionBehavior::Deny));
    assert!(agg.is_blocked());
    assert_eq!(
        agg.hook_permission_decision_reason.as_deref(),
        Some("dangerous")
    );
    assert_eq!(agg.additional_contexts, vec!["blocked cmd"]);
}

#[test]
fn test_aggregate_with_hook_specific_output_permission_ask() {
    let json_output = r#"{"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask", "permissionDecisionReason": "needs human", "additionalContext": "review cmd"}}"#;
    let results = vec![SingleHookResult {
        command: "check.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert_eq!(agg.permission_behavior, Some(PermissionBehavior::Ask));
    assert!(!agg.is_blocked());
    assert_eq!(
        agg.hook_permission_decision_reason.as_deref(),
        Some("needs human")
    );
    assert_eq!(agg.additional_contexts, vec!["review cmd"]);
}

#[test]
fn test_process_execution_result_prefers_stdout_json_on_nonzero_exit() {
    let result = process_execution_result(
        HookExecutionResult::CommandOutput {
            exit_code: 1,
            stdout: r#"{"additionalContext":"from stdout"}"#.to_string(),
            stderr: "stderr failure".to_string(),
        },
        "hook.sh",
        HookBlockingSource::Command("hook.sh".to_string()),
        HookEventType::PostToolUse,
        &coco_messages::AttachmentEmitter::noop(),
    );
    let agg = aggregate_results(&[result]);
    assert!(!agg.is_blocked());
    assert_eq!(agg.additional_contexts, vec!["from stdout"]);
}

#[test]
fn test_process_execution_result_exit_two_uses_stderr_without_stdout_json() {
    let result = process_execution_result(
        HookExecutionResult::CommandOutput {
            exit_code: 2,
            stdout: String::new(),
            stderr: "blocked via stderr".to_string(),
        },
        "hook.sh",
        HookBlockingSource::Command("hook.sh".to_string()),
        HookEventType::PostToolUse,
        &coco_messages::AttachmentEmitter::noop(),
    );
    let agg = aggregate_results(&[result]);
    assert!(agg.is_blocked());
    assert_eq!(
        agg.blocking_error
            .as_ref()
            .map(|e| e.blocking_error.as_str()),
        Some("blocked via stderr")
    );
}

#[test]
fn test_aggregate_elicitation_decline_blocks() {
    let json_output =
        r#"{"hookSpecificOutput": {"hookEventName": "Elicitation", "action": "decline"}}"#;
    let results = vec![SingleHookResult {
        command: "elicit.sh".to_string(),
        succeeded: true,
        output: json_output.to_string(),
        blocked: false,
        outcome: HookOutcome::Success,
        status_message: None,
        async_rewake: false,
        source: HookBlockingSource::Command(String::new()),
        sdk_output: None,
    }];
    let agg = aggregate_results(&results);
    assert!(agg.is_blocked());
    assert!(agg.elicitation_response.is_some());
    assert_eq!(
        agg.elicitation_response.as_ref().unwrap().action,
        ElicitationAction::Decline,
    );
}

// -----------------------------------------------------------------------
// Plugin env vars and CLAUDE_ENV_FILE tests
// -----------------------------------------------------------------------

#[test]
fn test_build_hook_env_with_plugin_context() {
    let mut opts = HashMap::new();
    opts.insert("api_key".to_string(), "secret".to_string());

    let ctx = HookPluginContext {
        plugin_root: Some("/plugins/my-plugin".to_string()),
        plugin_id: Some("my-plugin-id".to_string()),
        plugin_options: opts,
        skill_root: None,
    };

    let env = build_hook_env_with_plugin(
        "sess",
        "/cwd",
        None,
        HookEventType::PreToolUse,
        None,
        Some(&ctx),
        None,
    );
    assert_eq!(env.get("CLAUDE_PLUGIN_ROOT").unwrap(), "/plugins/my-plugin");
    assert!(
        env.get("CLAUDE_PLUGIN_DATA")
            .unwrap()
            .contains("my-plugin-id")
    );
    assert_eq!(env.get("CLAUDE_PLUGIN_OPTION_API_KEY").unwrap(), "secret");
}

#[test]
fn test_build_hook_env_claude_env_file_for_session_start() {
    // CLAUDE_ENV_FILE requires hook_index for uniqueness.
    let env = build_hook_env_with_plugin(
        "sess-1",
        "/cwd",
        None,
        HookEventType::SessionStart,
        None,
        None,
        Some(0),
    );
    assert!(env.contains_key("CLAUDE_ENV_FILE"));
    assert!(env.get("CLAUDE_ENV_FILE").unwrap().contains("sess-1"));
    assert!(env.get("CLAUDE_ENV_FILE").unwrap().contains("-0.sh"));
}

#[test]
fn test_build_hook_env_no_claude_env_file_without_hook_index() {
    // Without hook_index, CLAUDE_ENV_FILE is not set.
    let env = build_hook_env("sess-1", "/cwd", None, HookEventType::SessionStart, None);
    assert!(!env.contains_key("CLAUDE_ENV_FILE"));
}

#[test]
fn test_build_hook_env_no_claude_env_file_for_pre_tool_use() {
    let env = build_hook_env("sess-1", "/cwd", None, HookEventType::PreToolUse, None);
    assert!(!env.contains_key("CLAUDE_ENV_FILE"));
}

// -----------------------------------------------------------------------
// Formatting helpers
// -----------------------------------------------------------------------

#[test]
fn test_format_pre_tool_blocking_message() {
    let err = HookBlockingError {
        blocking_error: "write not allowed".to_string(),
        source: HookBlockingSource::Command("check-write.sh".to_string()),
    };
    let msg = format_pre_tool_blocking_message("PreToolUse:Write", &err);
    assert_eq!(msg, "PreToolUse:Write hook error: write not allowed");
}

#[test]
fn test_format_stop_hook_message() {
    let err = HookBlockingError {
        blocking_error: "tests failed".to_string(),
        source: HookBlockingSource::Command("run-tests.sh".to_string()),
    };
    let msg = format_stop_hook_message(&err);
    assert_eq!(msg, "Stop hook feedback:\ntests failed");
}

// ---------------------------------------------------------------------------
// resolve_timeout — handler-specific defaults
// ---------------------------------------------------------------------------

#[test]
fn resolve_timeout_agent_hook_defaults_to_60s() {
    // Agent hooks default to 60s, independent of the generic
    // 10-minute tool-hook timeout.
    let handler = HookHandler::Agent {
        prompt: "verify tests passed".into(),
        model: None,
        timeout_ms: None,
    };
    assert_eq!(
        resolve_timeout(&handler, DEFAULT_HOOK_TIMEOUT),
        Duration::from_secs(60)
    );
}

#[test]
fn resolve_timeout_prompt_hook_defaults_to_30s() {
    // Prompt hooks default to 30s.
    let handler = HookHandler::Prompt {
        prompt: "is this safe?".into(),
        model: None,
        timeout_ms: None,
    };
    assert_eq!(
        resolve_timeout(&handler, DEFAULT_HOOK_TIMEOUT),
        Duration::from_secs(30)
    );
}

#[test]
fn resolve_timeout_explicit_overrides_handler_default() {
    let handler = HookHandler::Agent {
        prompt: "verify".into(),
        model: None,
        timeout_ms: Some(5_000),
    };
    assert_eq!(
        resolve_timeout(&handler, DEFAULT_HOOK_TIMEOUT),
        Duration::from_millis(5_000)
    );
}

#[test]
fn resolve_timeout_command_hook_uses_event_default() {
    // Non-LLM handlers keep the event-supplied default.
    let handler = HookHandler::Command {
        command: "echo hi".into(),
        timeout_ms: None,
        shell: None,
    };
    assert_eq!(
        resolve_timeout(&handler, DEFAULT_HOOK_TIMEOUT),
        DEFAULT_HOOK_TIMEOUT
    );
    assert_eq!(
        resolve_timeout(&handler, SESSION_END_HOOK_TIMEOUT),
        SESSION_END_HOOK_TIMEOUT
    );
}
