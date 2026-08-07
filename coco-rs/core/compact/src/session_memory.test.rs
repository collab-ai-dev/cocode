use super::*;

/// Test helper: wrap a borrowed `&[Message]` into the canonical
/// `Vec<Arc<Message>>` shape that the post-refactor `compact_session_memory`
/// accepts. Caller keeps ownership of the source vec.
fn arc_vec(msgs: &[Message]) -> Vec<std::sync::Arc<Message>> {
    msgs.iter().cloned().map(std::sync::Arc::new).collect()
}

fn make_user_message(text: &str) -> Message {
    Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::User {
            content: vec![coco_messages::UserContent::Text(
                coco_llm_types::TextPart::new(text.to_string()),
            )],
            provider_options: None,
        },
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

fn make_assistant_message(text: &str) -> Message {
    Message::Assistant(coco_messages::AssistantMessage {
        message: coco_messages::LlmMessage::Assistant {
            content: vec![coco_messages::AssistantContent::Text(
                coco_llm_types::TextPart::new(text.to_string()),
            )],
            provider_options: None,
        },
        uuid: uuid::Uuid::new_v4(),
        model: "test-model".to_string(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn test_compact_session_memory_produces_summary() {
    let messages = vec![
        make_user_message("Hello, help me with this code"),
        make_assistant_message("Sure, I can help you refactor that module"),
        make_user_message("Great, let's start with the parser"),
        make_assistant_message("I'll restructure the parser into smaller functions"),
    ];
    let session_memory = "## Key Decisions\n- Refactored parser into smaller functions\n\n## Context\n- User working on parser module";
    let config = SessionMemoryCompactConfig::default();

    let result = compact_session_memory(&arc_vec(&messages), session_memory, None, &config)
        .expect("should not error")
        .expect("should produce a result");

    assert!(
        !result.summary_messages.is_empty(),
        "should have summary messages"
    );
    assert!(result.pre_compact_tokens > 0);
    // Boundary marker should be in the dedicated field, not in summary_messages
    assert!(matches!(
        result.boundary_marker,
        Message::System(coco_messages::SystemMessage::CompactBoundary(_))
    ));
    // summary_messages should contain only the user summary, not the boundary
    assert!(
        result
            .summary_messages
            .iter()
            .all(|m| matches!(m, Message::User(_)))
    );
}

#[test]
fn test_compact_session_memory_empty_returns_none() {
    let messages = vec![make_user_message("hello")];
    let config = SessionMemoryCompactConfig::default();

    let result =
        compact_session_memory(&arc_vec(&messages), "", None, &config).expect("should not error");
    assert!(result.is_none(), "empty session memory should return None");

    let result2 = compact_session_memory(&arc_vec(&messages), "   \n  ", None, &config)
        .expect("should not error");
    assert!(
        result2.is_none(),
        "whitespace-only session memory should return None"
    );
}

#[test]
fn test_select_memories_for_compaction_picks_stale() {
    let entries = vec![
        (
            "recent.md".to_string(),
            MemoryMetadata {
                staleness_days: 1,
                access_count: 10,
            },
        ),
        (
            "old.md".to_string(),
            MemoryMetadata {
                staleness_days: 30,
                access_count: 2,
            },
        ),
        (
            "medium.md".to_string(),
            MemoryMetadata {
                staleness_days: 15,
                access_count: 5,
            },
        ),
    ];

    let to_compact = select_memories_for_compaction(&entries, /*max_to_keep*/ 2);
    assert_eq!(to_compact.len(), 1);
    assert_eq!(
        to_compact[0], "old.md",
        "oldest entry should be selected first"
    );
}

#[test]
fn test_select_memories_under_limit_returns_empty() {
    let entries = vec![
        (
            "a.md".to_string(),
            MemoryMetadata {
                staleness_days: 1,
                access_count: 1,
            },
        ),
        (
            "b.md".to_string(),
            MemoryMetadata {
                staleness_days: 2,
                access_count: 1,
            },
        ),
    ];
    let to_compact = select_memories_for_compaction(&entries, /*max_to_keep*/ 5);
    assert!(to_compact.is_empty());
}

#[test]
fn test_merge_similar_memories_deduplicates() {
    let memories = vec![
        (
            "parser_v1.md".to_string(),
            "Use recursive descent\nHandle errors gracefully".to_string(),
        ),
        (
            "parser_v2.md".to_string(),
            "Use recursive descent\nAdd streaming support".to_string(),
        ),
    ];

    let merged = merge_similar_memories(&memories);
    assert_eq!(merged.len(), 1, "should merge into one entry");
    let (name, content) = &merged[0];
    assert_eq!(name, "parser");
    // "Use recursive descent" should appear only once
    assert_eq!(
        content.matches("Use recursive descent").count(),
        1,
        "duplicate lines should be deduplicated"
    );
    assert!(content.contains("Handle errors gracefully"));
    assert!(content.contains("Add streaming support"));
}

#[test]
fn test_merge_similar_memories_keeps_unique() {
    let memories = vec![
        (
            "auth_config.md".to_string(),
            "OAuth2 setup notes".to_string(),
        ),
        ("parser_rules.md".to_string(), "Grammar rules".to_string()),
    ];

    let merged = merge_similar_memories(&memories);
    assert_eq!(merged.len(), 2, "different prefixes should not be merged");
}

#[test]
fn test_extract_name_prefix() {
    assert_eq!(extract_name_prefix("parser_v1.md"), "parser");
    assert_eq!(extract_name_prefix("auth-config.md"), "auth");
    assert_eq!(extract_name_prefix("simple.md"), "simple");
    assert_eq!(extract_name_prefix("no_ext"), "no");
}

#[test]
fn test_should_extract_memory_init_below_threshold() {
    let t = SessionMemoryExtractionThresholds::default();
    assert!(!should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 5_000,
            tokens_at_last_extract: 0,
            tool_calls_in_last_turn: 0,
        },
        &t,
    ));
}

#[test]
fn test_should_extract_memory_init_meets_threshold() {
    let t = SessionMemoryExtractionThresholds::default();
    assert!(should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 10_000,
            tokens_at_last_extract: 0,
            tool_calls_in_last_turn: 0,
        },
        &t,
    ));
}

#[test]
fn test_should_extract_memory_update_blocked_by_small_delta() {
    let t = SessionMemoryExtractionThresholds::default();
    assert!(!should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 12_000,
            tokens_at_last_extract: 10_000,
            tool_calls_in_last_turn: 5,
        },
        &t,
    ));
}

#[test]
fn test_should_extract_memory_update_tool_burst_path() {
    let t = SessionMemoryExtractionThresholds::default();
    assert!(should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 16_000,
            tokens_at_last_extract: 10_000,
            tool_calls_in_last_turn: 4,
        },
        &t,
    ));
}

#[test]
fn test_should_extract_memory_update_idle_turn_path() {
    let t = SessionMemoryExtractionThresholds::default();
    // Tool calls = 0 (idle turn), but token delta high → extract.
    assert!(should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 16_000,
            tokens_at_last_extract: 10_000,
            tool_calls_in_last_turn: 0,
        },
        &t,
    ));
}

#[test]
fn test_should_extract_memory_update_low_tools_blocked() {
    let t = SessionMemoryExtractionThresholds::default();
    // Delta high but tool_calls between 1..min — blocked.
    assert!(!should_extract_memory(
        SessionMemoryExtractionInputs {
            current_tokens: 16_000,
            tokens_at_last_extract: 10_000,
            tool_calls_in_last_turn: 2,
        },
        &t,
    ));
}

#[test]
fn test_template_only_recognized() {
    use crate::session_memory::is_session_memory_template_only;
    // Pure heading template.
    assert!(is_session_memory_template_only(
        "# Session Memory\n\n## Decisions\n\n## Files\n"
    ));
    // Heading + "none yet" placeholders.
    assert!(is_session_memory_template_only(
        "## Decisions\n- _none yet_\n## Open Questions\n- no entries\n"
    ));
    // Empty / whitespace.
    assert!(is_session_memory_template_only(""));
    assert!(is_session_memory_template_only("   \n\n  "));
    assert!(is_session_memory_template_only("(empty)"));
    assert!(is_session_memory_template_only(
        "# Hi\nNo memories yet — try again later"
    ));
}

#[test]
fn test_template_only_rejects_real_content() {
    use crate::session_memory::is_session_memory_template_only;
    assert!(!is_session_memory_template_only(
        "## Decisions\n- Use BTreeMap for deterministic ordering\n"
    ));
    assert!(!is_session_memory_template_only("Just a sentence."));
}

#[test]
fn test_compact_session_memory_returns_none_for_template() {
    let messages = vec![make_user_message("hi"), make_assistant_message("hello")];
    let template = "# Session Memory\n\n## Decisions\n- _none yet_\n";
    let config = SessionMemoryCompactConfig::default();
    let result = compact_session_memory(&arc_vec(&messages), template, None, &config)
        .expect("should not error");
    assert!(
        result.is_none(),
        "template-only content must short-circuit to None"
    );
}

#[test]
fn test_compact_session_memory_unrecognized_anchor_returns_none() {
    // last_summarized_message_id present but absent from history → bail.
    let messages = vec![make_user_message("hi"), make_assistant_message("hello")];
    let stale = uuid::Uuid::new_v4();
    let config = SessionMemoryCompactConfig::default();
    let result = compact_session_memory(
        &arc_vec(&messages),
        "real summary content",
        Some(stale),
        &config,
    )
    .expect("should not error");
    assert!(
        result.is_none(),
        "unrecognized anchor must bail to LLM fallback"
    );
}
