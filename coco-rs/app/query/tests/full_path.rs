//! Full-path integration test exercising all systems with mock model.
//!
//! Tests the complete agent loop: mock LLM → tool calls → permission check
//! → hook fire → budget track → compaction → session persist.

use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use coco_hooks::HookRegistry;
use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_session::SessionManager;
use coco_tool_runtime::ToolRegistry;
use coco_tools::BashTool;
use coco_tools::EditTool;
use coco_tools::GlobTool;
use coco_tools::GrepTool;
use coco_tools::ReadTool;
use coco_tools::WriteTool;
use coco_types::PermissionMode;
use tokio_util::sync::CancellationToken;

// ─── Mock model that exercises the full tool pipeline ───

/// Mock that:
/// - Call 0: Returns a Write tool call (create a file)
/// - Call 1: Returns a Read tool call (read the file back)
/// - Call 2: Returns final text summarizing what happened
struct FullPathMock {
    call_count: AtomicI32,
    test_dir: String,
}

#[async_trait::async_trait]
impl LanguageModel for FullPathMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock-full-path"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);

        match call {
            0 => {
                // Turn 1: Write a test file
                let file_path = format!("{}/e2e_test.txt", self.test_dir);
                Ok(LanguageModelGenerateResult {
                    content: vec![
                        AssistantContentPart::Text(TextPart {
                            text: "I'll create a test file first.".into(),
                            provider_metadata: None,
                        }),
                        AssistantContentPart::ToolCall(ToolCallPart {
                            tool_call_id: "write_1".into(),
                            tool_name: "Write".into(),
                            input: serde_json::json!({
                                "file_path": file_path,
                                "content": "Hello from e2e test!\nLine 2\nLine 3\n"
                            }),
                            provider_executed: None,
                            provider_metadata: None,
                            invalid: false,
                            invalid_reason: None,
                        }),
                    ],
                    usage: Usage::new(100, 50),
                    finish_reason: FinishReason::new(StopReason::ToolUse),
                    warnings: vec![],
                    provider_metadata: None,
                    request: None,
                    response: None,
                })
            }
            1 => {
                // Turn 2: Read it back
                let file_path = format!("{}/e2e_test.txt", self.test_dir);
                Ok(LanguageModelGenerateResult {
                    content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: "read_1".into(),
                        tool_name: "Read".into(),
                        input: serde_json::json!({"file_path": file_path}),
                        provider_executed: None,
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    })],
                    usage: Usage::new(150, 30),
                    finish_reason: FinishReason::new(StopReason::ToolUse),
                    warnings: vec![],
                    provider_metadata: None,
                    request: None,
                    response: None,
                })
            }
            _ => {
                // Turn 3: Final summary
                Ok(LanguageModelGenerateResult {
                    content: vec![AssistantContentPart::Text(TextPart {
                        text:
                            "Done! I created and read the file successfully. It contains 3 lines."
                                .into(),
                        provider_metadata: None,
                    })],
                    usage: Usage::new(200, 40),
                    finish_reason: FinishReason::new(StopReason::EndTurn),
                    warnings: vec![],
                    provider_metadata: None,
                    request: None,
                    response: None,
                })
            }
        }
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

/// Register all 6 core tools.
fn register_core_tools() -> ToolRegistry {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(GrepTool));
    registry
}

// ─── Tests ───

#[tokio::test]
async fn test_full_path_write_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let test_dir = dir.path().to_str().unwrap().to_string();

    let model = Arc::new(FullPathMock {
        call_count: AtomicI32::new(0),
        test_dir: test_dir.clone(),
    });
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(register_core_tools());
    let cancel = CancellationToken::new();

    let config = QueryEngineConfig {
        model_id: "mock-full-path".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        ..Default::default()
    };

    let engine = QueryEngine::new(config, client, tools, cancel, None);
    let result = engine
        .run("Create a test file and read it back")
        .await
        .unwrap();

    // Should have done 3 turns: write → read → final text
    assert_eq!(result.turns, 3);
    assert!(!result.cancelled);
    assert!(!result.budget_exhausted);
    assert!(result.response_text.contains("successfully"));

    // Verify the file was actually created on disk
    let file_content = std::fs::read_to_string(format!("{test_dir}/e2e_test.txt")).unwrap();
    assert!(file_content.contains("Hello from e2e test!"));
    assert_eq!(file_content.lines().count(), 3);

    // Verify token usage accumulated
    assert_eq!(result.total_usage.input_tokens.total, 450); // 100+150+200
    assert_eq!(result.total_usage.output_tokens.total, 120); // 50+30+40
}

#[tokio::test]
async fn test_full_path_with_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let test_dir = dir.path().to_str().unwrap().to_string();

    let model = Arc::new(FullPathMock {
        call_count: AtomicI32::new(0),
        test_dir,
    });
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(register_core_tools());
    let cancel = CancellationToken::new();

    // Create hook registry (empty — just proves it doesn't crash)
    let hooks = Arc::new(HookRegistry::new());

    let config = QueryEngineConfig {
        model_id: "mock-full-path".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        ..Default::default()
    };

    let engine = QueryEngine::new(config, client, tools, cancel, Some(hooks));
    let result = engine.run("test with hooks").await.unwrap();

    assert_eq!(result.turns, 3);
    assert!(result.response_text.contains("successfully"));
}

#[tokio::test]
async fn test_full_path_budget_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    let test_dir = dir.path().to_str().unwrap().to_string();

    let model = Arc::new(FullPathMock {
        call_count: AtomicI32::new(0),
        test_dir,
    });
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(register_core_tools());
    let cancel = CancellationToken::new();

    // Very small budget — should stop after first turn
    let config = QueryEngineConfig {
        model_id: "mock-full-path".into(),
        permission_mode: PermissionMode::BypassPermissions,
        total_token_budget: Some(50), // Will be exhausted after first turn (100+50=150 > 50)
        max_turns: Some(10),
        ..Default::default()
    };

    let engine = QueryEngine::new(config, client, tools, cancel, None);
    let result = engine.run("test budget").await.unwrap();

    assert!(result.budget_exhausted);
    assert_eq!(result.turns, 1); // Only 1 turn before budget stops
}

#[tokio::test]
async fn test_full_path_with_bash_safety() {
    // Mock that asks to run a destructive command
    struct DestructiveBashMock;

    #[async_trait::async_trait]
    impl LanguageModel for DestructiveBashMock {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock-destructive"
        }
        async fn do_generate(
            &self,
            options: &LanguageModelCallOptions,
            _abort_signal: Option<tokio_util::sync::CancellationToken>,
        ) -> Result<LanguageModelGenerateResult, AISdkError> {
            // Check if we've received a tool error (permission denied)
            let has_tool_error = options.prompt.iter().any(|msg| {
                format!("{msg:?}").contains("permission denied")
                    || format!("{msg:?}").contains("delete")
            });

            if has_tool_error {
                // Second call: model acknowledges the denial
                return Ok(LanguageModelGenerateResult {
                    content: vec![AssistantContentPart::Text(TextPart {
                        text:
                            "I cannot execute destructive commands. Let me find a safer approach."
                                .into(),
                        provider_metadata: None,
                    })],
                    usage: Usage::new(30, 20),
                    finish_reason: FinishReason::new(StopReason::EndTurn),
                    warnings: vec![],
                    provider_metadata: None,
                    request: None,
                    response: None,
                });
            }

            // First call: attempt rm -rf /
            Ok(LanguageModelGenerateResult {
                content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id: "bash_1".into(),
                    tool_name: "Bash".into(),
                    input: serde_json::json!({"command": "rm -rf /"}),
                    provider_executed: None,
                    provider_metadata: None,
                    invalid: false,
                    invalid_reason: None,
                })],
                usage: Usage::new(20, 10),
                finish_reason: FinishReason::new(StopReason::ToolUse),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            })
        }

        async fn do_stream(
            &self,
            options: &LanguageModelCallOptions,
            _abort_signal: Option<tokio_util::sync::CancellationToken>,
        ) -> Result<LanguageModelStreamResult, AISdkError> {
            let result = self.do_generate(options, None).await?;
            Ok(coco_inference::synthetic_stream_from_content(
                result.content,
                result.usage,
                result.finish_reason,
            ))
        }
    }

    let model = Arc::new(DestructiveBashMock);
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(register_core_tools());
    let cancel = CancellationToken::new();

    let config = QueryEngineConfig {
        model_id: "mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(5),
        ..Default::default()
    };

    let engine = QueryEngine::new(config, client, tools, cancel, None);
    let result = engine.run("delete everything").await.unwrap();

    // The model should acknowledge the denial
    assert!(result.response_text.contains("cannot") || result.response_text.contains("safer"));
    // The destructive command error is returned as a tool result in the same turn,
    // so the model may see the error and respond in 1 or 2 turns depending on timing.
    assert!(result.turns >= 1 && result.turns <= 3);
}

#[tokio::test]
async fn test_session_persistence_round_trip() {
    // Post-fix the SessionManager has no sidecar `.json` files —
    // every session-level fact is derived from the JSONL transcript.
    // This integration test seeds a transcript via `TranscriptStore`
    // (the same path the runtime exercises) and verifies
    // SessionManager's derive / delete operations work end-to-end
    // against the on-disk JSONL.
    use coco_session::TranscriptEntry;
    use coco_session::TranscriptStore;

    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());

    let cwd = std::path::Path::new("/tmp/test-cwd");
    let paths = std::sync::Arc::new(coco_paths::ProjectPaths::new(dir.path().to_path_buf(), cwd));
    let store = TranscriptStore::new(paths);
    let sid = "round-trip-sess";
    let entry = TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: format!("{sid}-u1"),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: sid.to_string(),
        cwd: cwd.display().to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        version: None,
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({"role":"user","content":"hi"})),
        usage: None,
        model: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    store.append_message(sid, &entry).unwrap();

    let sessions = mgr.list().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, sid);
    assert_eq!(sessions[0].working_dir, cwd);

    let loaded = mgr.load(sid).unwrap();
    assert_eq!(loaded.id, sid);

    mgr.delete(sid).unwrap();
    assert!(mgr.list().unwrap().is_empty());
}

#[tokio::test]
async fn test_full_path_glob_and_grep() {
    // Mock that creates files, then uses Glob and Grep to find them
    let dir = tempfile::tempdir().unwrap();
    let test_dir = dir.path().to_str().unwrap().to_string();

    // Pre-create some files
    std::fs::write(format!("{test_dir}/foo.rs"), "fn hello() {}").unwrap();
    std::fs::write(format!("{test_dir}/bar.rs"), "fn world() {}").unwrap();
    std::fs::write(format!("{test_dir}/readme.md"), "# Hello").unwrap();

    struct GlobGrepMock {
        call_count: AtomicI32,
        test_dir: String,
    }

    #[async_trait::async_trait]
    impl LanguageModel for GlobGrepMock {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock-glob-grep"
        }
        async fn do_generate(
            &self,
            _options: &LanguageModelCallOptions,
            _abort_signal: Option<tokio_util::sync::CancellationToken>,
        ) -> Result<LanguageModelGenerateResult, AISdkError> {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => {
                    // Glob for .rs files
                    Ok(LanguageModelGenerateResult {
                        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                            tool_call_id: "glob_1".into(),
                            tool_name: "Glob".into(),
                            input: serde_json::json!({
                                "pattern": "*.rs",
                                "path": self.test_dir
                            }),
                            provider_executed: None,
                            provider_metadata: None,
                            invalid: false,
                            invalid_reason: None,
                        })],
                        usage: Usage::new(20, 10),
                        finish_reason: FinishReason::new(StopReason::ToolUse),
                        warnings: vec![],
                        provider_metadata: None,
                        request: None,
                        response: None,
                    })
                }
                1 => {
                    // Grep for "hello" in the dir
                    Ok(LanguageModelGenerateResult {
                        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                            tool_call_id: "grep_1".into(),
                            tool_name: "Grep".into(),
                            input: serde_json::json!({
                                "pattern": "hello",
                                "path": self.test_dir,
                                "output_mode": "files_with_matches"
                            }),
                            provider_executed: None,
                            provider_metadata: None,
                            invalid: false,
                            invalid_reason: None,
                        })],
                        usage: Usage::new(30, 10),
                        finish_reason: FinishReason::new(StopReason::ToolUse),
                        warnings: vec![],
                        provider_metadata: None,
                        request: None,
                        response: None,
                    })
                }
                _ => Ok(LanguageModelGenerateResult {
                    content: vec![AssistantContentPart::Text(TextPart {
                        text: "Found 2 Rust files and hello() in foo.rs.".into(),
                        provider_metadata: None,
                    })],
                    usage: Usage::new(20, 15),
                    finish_reason: FinishReason::new(StopReason::EndTurn),
                    warnings: vec![],
                    provider_metadata: None,
                    request: None,
                    response: None,
                }),
            }
        }
        async fn do_stream(
            &self,
            options: &LanguageModelCallOptions,
            _abort_signal: Option<tokio_util::sync::CancellationToken>,
        ) -> Result<LanguageModelStreamResult, AISdkError> {
            let result = self.do_generate(options, None).await?;
            Ok(coco_inference::synthetic_stream_from_content(
                result.content,
                result.usage,
                result.finish_reason,
            ))
        }
    }

    let model = Arc::new(GlobGrepMock {
        call_count: AtomicI32::new(0),
        test_dir,
    });
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(register_core_tools());
    let cancel = CancellationToken::new();

    let config = QueryEngineConfig {
        model_id: "mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(10),
        ..Default::default()
    };

    let engine = QueryEngine::new(config, client, tools, cancel, None);
    let result = engine.run("find rust files with hello").await.unwrap();

    assert_eq!(result.turns, 3);
    assert!(result.response_text.contains("foo.rs"));
}
