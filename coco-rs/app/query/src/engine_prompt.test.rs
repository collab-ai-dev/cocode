//! Unit tests for the budget CoW helper —
//! `rewrite_tool_result_to_placeholder`. The function is the
//! hot-path optimization that avoids cloning a huge tool result
//! body just to immediately overwrite it.

use std::sync::Arc;
use uuid::Uuid;

use coco_llm_types::LlmMessage;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultPart;
use coco_llm_types::UserContentPart;
use coco_types::messages::{Message, ToolResultMessage};

use crate::QueryEngineConfig;

use super::*;

#[test]
fn system_prompt_context_message_carries_prompt_parts_metadata() {
    let prompt_context =
        coco_context::PromptContext::build(coco_context::PromptContextMode::literal(
            coco_context::PromptContextLiteralSource::ConfigOverride,
            "system body",
        ));

    let message = system_message_from_prompt_context(&prompt_context);

    let LlmMessage::System {
        content,
        provider_options,
    } = message
    else {
        panic!("expected system message");
    };
    let UserContentPart::Text(text) = &content[0] else {
        panic!("expected text system part");
    };
    assert_eq!(text.text, "system body");

    let namespace = provider_options
        .as_ref()
        .and_then(|opts| opts.get(coco_inference::PROMPT_LAYOUT_NAMESPACE))
        .expect("prompt_layout namespace");
    let parts = namespace
        .get("system_parts")
        .and_then(|v| v.as_array())
        .expect("system_parts array");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "system body");
    assert_eq!(parts[0]["cache_hint"], "ephemeral");
}

fn make_tool_result(
    tool_use_id: &str,
    body: &str,
    is_error: bool,
    provider_metadata: Option<coco_llm_types::ProviderMetadata>,
) -> ToolResultMessage {
    ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart {
                tool_call_id: tool_use_id.to_string(),
                tool_name: "Bash".to_string(),
                output: if is_error {
                    ToolResultContent::error_text(body)
                } else {
                    ToolResultContent::text(body)
                },
                is_error,
                provider_metadata,
            })],
            provider_options: None,
        },
        tool_use_id: tool_use_id.to_string(),
        tool_id: "Bash".parse().unwrap(),
        is_error,
    }
}

#[test]
fn rewrite_replaces_body_and_preserves_metadata() {
    let orig = make_tool_result("tu_1", "very long original output", false, None);
    let new =
        rewrite_tool_result_to_placeholder(&orig, "<persisted-output>preview</persisted-output>")
            .expect("rewrite must succeed for a well-formed tool result");

    // Outer envelope fields preserved.
    assert_eq!(new.uuid, orig.uuid);
    assert_eq!(new.tool_use_id, orig.tool_use_id);
    assert_eq!(new.tool_id, orig.tool_id);
    assert_eq!(new.is_error, orig.is_error);

    // Inner block: new body, but tool_name and other metadata preserved.
    let LlmMessage::Tool { content, .. } = &new.message else {
        panic!("expected LlmMessage::Tool");
    };
    assert_eq!(content.len(), 1);
    let ToolContentPart::ToolResult(part) = &content[0] else {
        panic!("expected ToolContentPart::ToolResult");
    };
    assert_eq!(part.tool_call_id, "tu_1");
    assert_eq!(part.tool_name, "Bash");
    assert!(!part.is_error);
    let ToolResultContent::Text { value, .. } = &part.output else {
        panic!("expected text output");
    };
    assert_eq!(value, "<persisted-output>preview</persisted-output>");
}

#[test]
fn rewrite_preserves_error_flag_via_error_text() {
    // is_error=true must round-trip into ToolResultContent::ErrorText so
    // the provider sees a typed error result, not a plain text result.
    let orig = make_tool_result("tu_2", "stderr noise", true, None);
    let new = rewrite_tool_result_to_placeholder(&orig, "REDACTED").unwrap();

    assert!(new.is_error);
    let LlmMessage::Tool { content, .. } = &new.message else {
        panic!();
    };
    let ToolContentPart::ToolResult(part) = &content[0] else {
        panic!();
    };
    assert!(part.is_error);
    let ToolResultContent::ErrorText { value, .. } = &part.output else {
        panic!("expected error_text output for is_error=true");
    };
    assert_eq!(value, "REDACTED");
}

#[test]
fn rewrite_returns_none_for_non_tool_inner_message() {
    // Defensive: if the inner LlmMessage isn't Tool, we shouldn't
    // silently rewrite something else. Caller falls back to legacy
    // clone-then-mutate path.
    let mut orig = make_tool_result("tu_3", "ok", false, None);
    orig.message = LlmMessage::user_text("not a tool result");

    assert!(rewrite_tool_result_to_placeholder(&orig, "ignored").is_none());
}

#[test]
fn rewrite_returns_none_when_tool_use_id_doesnt_match_any_block() {
    // If no inner block matches `orig.tool_use_id` we'd be writing into
    // nothing — return None and let the fallback handle it.
    let mut orig = make_tool_result("tu_real", "ok", false, None);
    orig.tool_use_id = "tu_mismatch".into();

    assert!(rewrite_tool_result_to_placeholder(&orig, "ignored").is_none());
}

#[test]
fn rewrite_does_not_copy_the_old_body_into_the_new_message() {
    // The whole point of this helper: the discarded `output.value`
    // never appears in the rebuilt Message. Construct a body big
    // enough to be distinctive and assert the replacement text is
    // what shows up — and the original text is gone.
    let huge_body = "BIG_ORIGINAL_PAYLOAD_".repeat(4096); // ~80 KB
    let orig = make_tool_result("tu_huge", &huge_body, false, None);
    let new =
        rewrite_tool_result_to_placeholder(&orig, "<persisted-output>tiny</persisted-output>")
            .unwrap();

    let LlmMessage::Tool { content, .. } = &new.message else {
        panic!();
    };
    let ToolContentPart::ToolResult(part) = &content[0] else {
        panic!();
    };
    let ToolResultContent::Text { value, .. } = &part.output else {
        panic!();
    };
    assert!(
        !value.contains("BIG_ORIGINAL_PAYLOAD"),
        "rewritten output must not carry the original body — \
         the helper exists to avoid the memcpy"
    );
    assert_eq!(value, "<persisted-output>tiny</persisted-output>");
}

#[test]
fn rewrite_into_arc_message_preserves_original_arc_in_history() {
    // Mirrors the budget CoW production path: history holds the
    // original Arc<Message>, working copy gets a fresh Arc with the
    // rewrite. Verifies the two Arcs are distinct allocations AND the
    // original still carries the verbose body (so MessageHistory /
    // transcript / resume keep full fidelity).
    let orig = make_tool_result("tu_a", "FULL VERBOSE BODY", false, None);
    let history_arc: Arc<Message> = Arc::new(Message::ToolResult(orig));

    // Simulated working-copy rewrite (same shape as
    // apply_tool_result_budget_to_prompt's loop).
    let Message::ToolResult(orig_inner) = history_arc.as_ref() else {
        panic!();
    };
    let new_inner =
        rewrite_tool_result_to_placeholder(orig_inner, "PLACEHOLDER").expect("rewrite ok");
    let working_arc: Arc<Message> = Arc::new(Message::ToolResult(new_inner));

    // Two distinct Arcs — replacing the slot in working copy doesn't
    // touch the history allocation.
    assert!(!Arc::ptr_eq(&history_arc, &working_arc));

    // History still has verbose body.
    let Message::ToolResult(hist) = history_arc.as_ref() else {
        panic!();
    };
    let LlmMessage::Tool { content, .. } = &hist.message else {
        panic!();
    };
    let ToolContentPart::ToolResult(part) = &content[0] else {
        panic!();
    };
    let ToolResultContent::Text { value, .. } = &part.output else {
        panic!();
    };
    assert_eq!(value, "FULL VERBOSE BODY");

    // Working copy has placeholder.
    let Message::ToolResult(wc) = working_arc.as_ref() else {
        panic!();
    };
    let LlmMessage::Tool { content, .. } = &wc.message else {
        panic!();
    };
    let ToolContentPart::ToolResult(part) = &content[0] else {
        panic!();
    };
    let ToolResultContent::Text { value, .. } = &part.output else {
        panic!();
    };
    assert_eq!(value, "PLACEHOLDER");
}

#[test]
fn builtin_mcp_boundary_marks_last_builtin_only_when_mcp_follows() {
    // Mixed set: built-ins [0..3) then MCP [3..5) ⇒ boundary on the last
    // built-in (index 2).
    assert_eq!(builtin_mcp_boundary_idx(3, 5), Some(2));
    // Single built-in followed by MCP ⇒ index 0.
    assert_eq!(builtin_mcp_boundary_idx(1, 4), Some(0));
}

#[test]
fn builtin_mcp_boundary_is_none_without_a_real_split() {
    // All built-in (no MCP tail to protect) ⇒ no breakpoint.
    assert_eq!(builtin_mcp_boundary_idx(5, 5), None);
    // All MCP (empty built-in prefix) ⇒ no breakpoint, and crucially must NOT
    // evaluate `0 - 1` (the `then_some`-eager-eval underflow that previously
    // panicked the no-built-in test paths).
    assert_eq!(builtin_mcp_boundary_idx(0, 4), None);
    // Empty tool set ⇒ no breakpoint, no underflow.
    assert_eq!(builtin_mcp_boundary_idx(0, 0), None);
}

/// Engine over an empty model-runtime registry — `observe_date_change` never
/// reaches a model, so no stub `LanguageModel` is needed.
fn date_latch_engine(
    config: QueryEngineConfig,
    app_state: &Arc<tokio::sync::RwLock<coco_types::ToolAppState>>,
) -> QueryEngine {
    let registry = Arc::new(
        coco_inference::ModelRuntimeRegistry::from_prebuilt_role_runtimes(Vec::<(
            coco_types::ModelRole,
            coco_inference::ModelRuntime,
        )>::new()),
    );
    QueryEngine::new(
        config,
        coco_types::SessionId::try_new("date-latch-session").unwrap(),
        registry,
        Arc::new(coco_tool_runtime::ToolRegistry::new()),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .with_app_state(Arc::clone(app_state))
}

/// The rollover latch is per visibility scope, and cache-shared forks sit out.
///
/// Regression: with one shared latch, whichever engine ran first after
/// midnight consumed the rollover and every other engine on the same
/// session-global `ToolAppState` silently never emitted its one-shot
/// `date_change` reminder — a background fork could swallow the main
/// conversation's notice.
#[tokio::test]
async fn date_change_latch_is_per_scope_and_forks_never_consume_it() {
    const STALE: &str = "1970-01-01";
    let app_state = Arc::new(tokio::sync::RwLock::new(coco_types::ToolAppState::default()));
    {
        let mut guard = app_state.write().await;
        guard.set_last_emitted_date_for_scope(None, STALE.to_string());
        guard.set_last_emitted_date_for_scope(Some("agent-a"), STALE.to_string());
    }

    // A cache-shared fork observes first: no reminder, and the main scope's
    // latch is left untouched for the main thread to consume.
    let fork = date_latch_engine(
        QueryEngineConfig {
            fork_label: Some(coco_types::ForkLabel::AutoDream),
            ..Default::default()
        },
        &app_state,
    );
    assert_eq!(fork.observe_date_change().await, None);
    assert_eq!(
        app_state
            .read()
            .await
            .last_emitted_date_for_scope(None)
            .as_deref(),
        Some(STALE),
    );

    // A subagent gets its own rollover notice and consumes only its own scope.
    let subagent = date_latch_engine(
        QueryEngineConfig {
            agent_id: Some(coco_types::AgentId::try_new("agent-a").unwrap()),
            ..Default::default()
        },
        &app_state,
    );
    let subagent_date = subagent
        .observe_date_change()
        .await
        .expect("subagent scope rolled over");
    assert_ne!(subagent_date, STALE);
    assert_eq!(
        app_state
            .read()
            .await
            .last_emitted_date_for_scope(Some("agent-a")),
        Some(subagent_date),
    );
    assert_eq!(
        app_state
            .read()
            .await
            .last_emitted_date_for_scope(None)
            .as_deref(),
        Some(STALE),
        "subagent must not consume the main thread's rollover",
    );

    // The main thread still gets its notice — exactly once.
    let main = date_latch_engine(QueryEngineConfig::default(), &app_state);
    let main_date = main
        .observe_date_change()
        .await
        .expect("main scope rolled over");
    assert_ne!(main_date, STALE);
    assert_eq!(main.observe_date_change().await, None);
}
