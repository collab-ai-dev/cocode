use super::*;

#[test]
fn prompt_includes_previous_when_provided() {
    let (sys, user) = build_summary_prompts("general-purpose", Some("Reading foo.ts"));
    assert!(sys.is_empty());
    assert!(user.contains("Previous: \"Reading foo.ts\""));
    assert!(user.contains("say something NEW"));
}

#[test]
fn prompt_omits_previous_when_none() {
    let (_sys, user) = build_summary_prompts("general-purpose", None);
    assert!(!user.contains("Previous:"));
    assert!(user.contains("Describe your most recent action"));
}

#[test]
fn prompt_body_has_no_leading_indent() {
    // `agentSummary.ts::buildSummaryPrompt` is a flat template
    // literal — every line starts at column 0. Byte parity matters
    // because the user prompt feeds into the parent's prompt cache
    // identity; a stray indent on each line shifts every byte and
    // busts the cache. The previous Rust impl used `\` continuations
    // with rustfmt indentation, which produced lines like
    // `         Good: "Reading runAgent.ts"` — wrong.
    let (_, user) = build_summary_prompts("general-purpose", None);
    for line in user.lines() {
        assert!(
            !line.starts_with(' '),
            "summary prompt line must not have leading whitespace: {line:?}"
        );
    }
}

#[test]
fn prompt_uses_tsfaithful_em_dash_in_previous_marker() {
    // U+2014 EM DASH between `…"` and `say something NEW.`. Verify
    // the exact codepoint round-trips so cache keys match across
    // runtimes.
    let (_, user) = build_summary_prompts("general-purpose", Some("Reading foo.ts"));
    assert!(user.contains("Previous: \"Reading foo.ts\" \u{2014} say something NEW."));
}

#[test]
fn sanitize_strips_quotes_and_whitespace() {
    assert_eq!(
        sanitize_summary("  \"Reading runAgent.ts\"  "),
        Some("Reading runAgent.ts".to_string())
    );
}

#[test]
fn sanitize_rejects_empty() {
    assert!(sanitize_summary("").is_none());
    assert!(sanitize_summary("   ").is_none());
    assert!(sanitize_summary("\"\"").is_none());
}

#[test]
fn sanitize_rejects_none_marker() {
    assert!(sanitize_summary("NONE").is_none());
    assert!(sanitize_summary("none").is_none());
    assert!(sanitize_summary("None").is_none());
}

#[test]
fn sanitize_rejects_overlong() {
    let long = "a".repeat(81);
    assert!(sanitize_summary(&long).is_none());
    let exact = "a".repeat(80);
    assert_eq!(sanitize_summary(&exact), Some(exact));
}

#[test]
fn should_summarize_gates_below_three_messages() {
    assert!(!should_summarize(0));
    assert!(!should_summarize(2));
    assert!(should_summarize(3));
    assert!(should_summarize(10));
}

fn user_msg(text: &str) -> std::sync::Arc<coco_types::messages::Message> {
    use coco_types::messages::{Message, UserMessage};
    std::sync::Arc::new(Message::User(UserMessage {
        message: coco_llm_types::LlmMessage::user_text(text),
        uuid: uuid::Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    }))
}

#[test]
fn render_transcript_tail_emits_role_lines_and_omits_tool_bodies() {
    use coco_llm_types::{
        AssistantContentPart, LlmMessage, ToolCallPart, ToolContentPart, ToolResultContent,
        ToolResultPart,
    };
    use coco_types::messages::{AssistantMessage, Message, ToolResultMessage};

    let messages = vec![
        user_msg("do the thing"),
        std::sync::Arc::new(Message::Assistant(AssistantMessage {
            message: LlmMessage::Assistant {
                content: vec![
                    AssistantContentPart::text("Let me search"),
                    AssistantContentPart::ToolCall(ToolCallPart::new(
                        "tu_1",
                        "Grep",
                        serde_json::Value::Null,
                    )),
                ],
                provider_options: None,
            },
            uuid: uuid::Uuid::new_v4(),
            model: String::new(),
            stop_reason: None,
            usage: None,
            cost_usd: None,
            request_id: None,
            api_error: None,
        })),
        std::sync::Arc::new(Message::ToolResult(ToolResultMessage {
            uuid: uuid::Uuid::new_v4(),
            source_assistant_uuid: None,
            display_data: None,
            message: LlmMessage::Tool {
                content: vec![ToolContentPart::ToolResult(ToolResultPart::new(
                    "tu_1",
                    "Grep",
                    ToolResultContent::text("file1.rs\nfile2.rs"),
                ))],
                provider_options: None,
            },
            tool_use_id: "tu_1".into(),
            tool_id: "Grep".parse().unwrap(),
            is_error: false,
        })),
    ];

    let out = render_transcript_tail(&messages, 4_000);
    assert!(out.contains("[user] do the thing"));
    assert!(out.contains("[assistant] Let me search"));
    assert!(out.contains("[assistant] tool_use: Grep"));
    assert!(out.contains("[user] tool_result"));
    // Tool-result body is omitted.
    assert!(!out.contains("file1.rs"));
}

#[test]
fn render_transcript_tail_snaps_multibyte_boundary_keeping_content() {
    // A cap landing mid-emoji must snap *back* to a char boundary: never
    // panic, never collapse to empty, and keep at least `max_chars` bytes.
    // (A forward scan could skip past the end and return "".)
    let messages = vec![user_msg(&"🔥".repeat(50))];
    let full = render_transcript_tail(&messages, usize::MAX);
    let out = render_transcript_tail(&messages, 10);
    assert!(!out.is_empty(), "must not collapse to empty");
    assert!(out.len() >= 10, "backward snap keeps >= max_chars bytes");
    assert!(
        full.ends_with(&out),
        "result must be a valid suffix of the full render"
    );
}

#[test]
fn render_transcript_tail_keeps_the_trailing_window() {
    // Each line is "[user] <text>\n". Build enough lines to exceed the cap,
    // then assert the *tail* survives and the head is dropped.
    let messages: Vec<_> = (0..200).map(|i| user_msg(&format!("line{i}"))).collect();
    let out = render_transcript_tail(&messages, 200);
    assert!(out.len() <= 200);
    assert!(out.contains("line199"), "tail should be retained: {out}");
    assert!(!out.contains("[user] line0\n"), "head should be dropped");
}
