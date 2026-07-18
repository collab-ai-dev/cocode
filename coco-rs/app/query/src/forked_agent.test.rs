// Reads the process cwd, legitimate outside session-owned code; opts out of
// the workspace-wide `std::env::current_dir` session-cwd discipline gate.
#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use coco_messages::Message;
use coco_types::CacheSafeParams;
use coco_types::CacheTtl;
use coco_types::ForkLabel;
use coco_types::PromptCacheConfig;
use coco_types::PromptCacheMode;
use coco_types::TokenUsage;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;

fn test_session_id() -> coco_types::SessionId {
    coco_types::SessionId::try_new("test-session").unwrap()
}

#[test]
fn test_for_label_is_cache_safe() {
    // The cache-safe defaults are what promptSuggestion / session_memory
    // expect: 1 turn, skip transcript, skip cache
    // write, no effort override (PR #18143 cache-bust risk).
    let opts = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);
    assert_eq!(opts.max_turns, Some(1));
    assert_eq!(opts.transcript_mode, ForkTranscriptMode::Disabled);
    assert!(opts.skip_cache_write);
    assert!(
        opts.effort.is_none(),
        "effort override busts cache; default must be None"
    );
    assert!(opts.can_use_tool.is_none());
    assert!(!opts.require_can_use_tool);
}

#[test]
fn test_for_label_query_source_matches_label_str() {
    // Every variant's query_source defaults to label.as_str() so
    // telemetry pivots align with the typed enum without manual
    // string drift.
    let cases = [
        (ForkLabel::PromptSuggestion, "prompt_suggestion"),
        (ForkLabel::Compact, "compact"),
        (ForkLabel::ExtractMemories, "extract_memories"),
        (ForkLabel::SessionMemoryAuto, "session_memory_auto"),
        (ForkLabel::SessionMemoryManual, "session_memory_manual"),
        (ForkLabel::AgentSummary, "agent_summary"),
        (ForkLabel::AutoDream, "auto_dream"),
        (ForkLabel::Speculation, "speculation"),
        (ForkLabel::HookAgent, "hook_agent"),
    ];
    for (label, wire) in cases {
        let opts = ForkedAgentOptions::for_label(label);
        assert_eq!(opts.query_source, wire, "query_source for {label:?}");
        assert_eq!(opts.fork_label, label);
    }
}

#[test]
fn test_for_label_carries_can_use_tool() {
    let mut opts = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);
    opts.can_use_tool = Some(deny_all_handle("test"));
    assert!(opts.can_use_tool.is_some());
}

#[test]
fn test_build_query_config_inherits_prompt_cache_and_sets_skip_cache_write() {
    let cache = CacheSafeParams {
        rendered_system_prompt: "system".into(),
        model_id: "claude-opus-4-7".into(),
        provider: "anthropic".into(),
        active_shell_tool: coco_types::ActiveShellTool::Disabled,
        prompt_cache: Some(PromptCacheConfig {
            mode: PromptCacheMode::Auto,
            ttl: CacheTtl::OneHour,
            scope: None,
            requested_betas: Default::default(),
            skip_cache_write: false,
        }),
        effort: None,
        fork_context_messages: vec![Arc::new(coco_messages::create_user_message("parent turn"))],
    };
    let options = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);

    let config = build_query_config(&cache, &options, &test_session_id());

    let prompt_cache = config
        .prompt_cache
        .expect("parent prompt-cache directive should be inherited");
    assert_eq!(prompt_cache.mode, PromptCacheMode::Auto);
    assert_eq!(prompt_cache.ttl, CacheTtl::OneHour);
    assert!(
        prompt_cache.skip_cache_write,
        "fire-and-forget fork must flip skip_cache_write without losing cache-key fields"
    );
    assert_eq!(config.fork_context_messages.len(), 1);
    assert_eq!(
        config.active_shell_tool,
        coco_types::ActiveShellTool::Disabled
    );
    assert!(Arc::ptr_eq(
        &config.fork_context_messages[0],
        &cache.fork_context_messages[0],
    ));
}

/// Thinking parity: a cache-sharing fork mirrors the parent's captured
/// effective effort so its wire thinking params match the parent's —
/// thinking config keys Anthropic's messages-level cache (PR #18143).
#[test]
fn test_build_query_config_mirrors_parent_effective_effort() {
    fn cache_with_effort(effort: Option<coco_types::ReasoningEffort>) -> CacheSafeParams {
        CacheSafeParams {
            rendered_system_prompt: "system".into(),
            model_id: "claude-opus-4-7".into(),
            provider: "anthropic".into(),
            active_shell_tool: coco_types::ActiveShellTool::Disabled,
            prompt_cache: None,
            effort,
            fork_context_messages: Vec::new(),
        }
    }

    // Parent ran at High (Ctrl+T or `models.main.<slot>.effort`) →
    // the fork mirrors it.
    let options = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);
    let config = build_query_config(
        &cache_with_effort(Some(coco_types::ReasoningEffort::High)),
        &options,
        &test_session_id(),
    );
    assert_eq!(config.effort, Some(coco_types::ReasoningEffort::High));

    // Parent ran on the model default → fork stays None (same model ⇒
    // same default ⇒ parity without a snapshot).
    let config = build_query_config(&cache_with_effort(None), &options, &test_session_id());
    assert_eq!(config.effort, None);

    // An explicit per-fork override (deliberately cache-busting) still
    // wins over the parent snapshot.
    let mut options = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);
    options.effort = Some(coco_types::ReasoningEffort::Low);
    let config = build_query_config(
        &cache_with_effort(Some(coco_types::ReasoningEffort::High)),
        &options,
        &test_session_id(),
    );
    assert_eq!(config.effort, Some(coco_types::ReasoningEffort::Low));
}

#[tokio::test]
async fn test_deny_all_handle_round_trip() {
    let handle = deny_all_handle("prompt_suggestion: tools disabled");
    let ctx = CanUseToolCallContext {
        tool_use_id: "tu-1".into(),
        cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
        abort: coco_tool_runtime::TurnAbortSignal::from_token(CancellationToken::new()),
        require_can_use_tool: false,
        messages: Arc::new(Vec::<Arc<Message>>::new()),
    };
    let decision = handle
        .check(
            &"Bash".parse::<coco_types::ToolId>().unwrap(),
            "Bash",
            &json!({"command": "ls"}),
            &ctx,
        )
        .await;
    match decision {
        CanUseToolDecision::Deny { message, .. } => {
            assert!(
                message.contains("prompt_suggestion: tools disabled"),
                "deny message should carry caller-supplied reason: {message}"
            );
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[test]
fn test_forked_agent_result_default() {
    let r = ForkedAgentResult::default();
    assert!(r.messages.is_empty());
    assert_eq!(r.total_usage, TokenUsage::default());
}
