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

#[test]
fn test_for_label_is_cache_safe() {
    // The cache-safe defaults are what `/btw` / promptSuggestion /
    // session_memory expect: 1 turn, skip transcript, skip cache
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
        (ForkLabel::SideQuestion, "side_question"),
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
        fork_context_messages: vec![Arc::new(coco_messages::create_user_message("parent turn"))],
    };
    let options = ForkedAgentOptions::for_label(ForkLabel::PromptSuggestion);

    let config = build_query_config(&cache, &options, "test-session");

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

#[tokio::test]
async fn test_deny_all_handle_round_trip() {
    let handle = deny_all_handle("prompt_suggestion: tools disabled");
    let ctx = CanUseToolCallContext {
        tool_use_id: "tu-1".into(),
        abort: coco_tool_runtime::TurnAbortSignal::from_token(CancellationToken::new()),
        require_can_use_tool: false,
        messages: Arc::new(Vec::<Arc<Message>>::new()),
    };
    let decision = handle.check("Bash", &json!({"command": "ls"}), &ctx).await;
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
