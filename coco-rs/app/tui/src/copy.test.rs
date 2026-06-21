use std::sync::Arc;

use coco_messages::ApiError;
use coco_messages::AssistantContent;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::StopReason;
use coco_messages::TextContent;
use coco_messages::UserMessage;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::CodeBlock;
use super::MAX_LOOKBACK;
use super::collect_recent_assistant_texts;
use super::extract_code_blocks;
use super::file_extension;
use crate::state::transcript_view::TranscriptView;

fn assistant_with_parts(parts: Vec<AssistantContent>) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: parts,
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn text_part(text: &str) -> AssistantContent {
    AssistantContent::Text(TextContent {
        text: text.into(),
        provider_metadata: None,
    })
}

fn assistant_text(text: &str) -> Message {
    assistant_with_parts(vec![text_part(text)])
}

fn assistant_with_api_error(text: &str) -> Message {
    let mut msg = assistant_text(text);
    if let Message::Assistant(ref mut a) = msg {
        a.api_error = Some(ApiError {
            message: "boom".into(),
            status_code: Some(500),
            error_type: None,
        });
    }
    msg
}

fn user_text(text: &str) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![coco_messages::UserContent::text(text)],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn transcript_from(messages: Vec<Message>) -> TranscriptView {
    let mut view = TranscriptView::new();
    for msg in messages {
        view.on_message_appended(Arc::new(msg));
    }
    view
}

#[test]
fn extract_code_blocks_picks_up_single_block_with_lang() {
    let md = "intro\n```python\nprint('hi')\n```\noutro";
    let blocks = extract_code_blocks(md);
    assert_eq!(
        blocks,
        vec![CodeBlock {
            code: "print('hi')".into(),
            lang: Some("python".into()),
        }],
    );
}

#[test]
fn extract_code_blocks_handles_missing_lang() {
    let md = "```\nplain\n```";
    let blocks = extract_code_blocks(md);
    assert_eq!(
        blocks,
        vec![CodeBlock {
            code: "plain".into(),
            lang: None,
        }],
    );
}

#[test]
fn extract_code_blocks_collects_multiple_blocks() {
    let md = "```ts\nlet x = 1;\n```\nbetween\n```rust\nfn f() {}\n```";
    let blocks = extract_code_blocks(md);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].lang.as_deref(), Some("ts"));
    assert_eq!(blocks[0].code, "let x = 1;");
    assert_eq!(blocks[1].lang.as_deref(), Some("rust"));
    assert_eq!(blocks[1].code, "fn f() {}");
}

#[test]
fn extract_code_blocks_returns_empty_when_no_fences() {
    assert!(extract_code_blocks("just prose, no fences here").is_empty());
}

#[test]
fn extract_code_blocks_closes_unterminated_block_at_eof() {
    // marked.js auto-closes unterminated fences; we mirror that so the
    // last block doesn't silently disappear.
    let md = "```go\nfunc main() {}\nno closing fence";
    let blocks = extract_code_blocks(md);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].lang.as_deref(), Some("go"));
    assert_eq!(blocks[0].code, "func main() {}\nno closing fence");
}

#[test]
fn extract_code_blocks_strips_prompt_xml_tags_like_ts() {
    let md = "before\n<context>\n```rust\nhidden()\n```\n</context>\nafter";
    let blocks = extract_code_blocks(md);
    assert!(blocks.is_empty());
}

#[test]
fn extract_code_blocks_accepts_commonmark_fence_variants() {
    let md = "   ~~~ts\nconst x = 1\n~~~";
    let blocks = extract_code_blocks(md);
    assert_eq!(
        blocks,
        vec![CodeBlock {
            code: "const x = 1".into(),
            lang: Some("ts".into()),
        }],
    );
}

#[test]
fn file_extension_returns_txt_for_none_and_plaintext() {
    assert_eq!(file_extension(None), ".txt");
    assert_eq!(file_extension(Some("plaintext")), ".txt");
    assert_eq!(file_extension(Some("")), ".txt");
}

#[test]
fn file_extension_sanitizes_path_traversal_attempts() {
    assert_eq!(file_extension(Some("../../etc/passwd")), ".etcpasswd");
    assert_eq!(file_extension(Some("ts.x")), ".tsx");
}

#[test]
fn file_extension_passes_simple_lang() {
    assert_eq!(file_extension(Some("python")), ".python");
    assert_eq!(file_extension(Some("rust")), ".rust");
}

#[test]
fn collect_returns_latest_first_and_skips_users() {
    let view = transcript_from(vec![
        user_text("hi"),
        assistant_text("first reply"),
        user_text("again"),
        assistant_text("second reply"),
    ]);
    let texts = collect_recent_assistant_texts(&view, 10);
    assert_eq!(texts, vec!["second reply", "first reply"]);
}

#[test]
fn collect_skips_api_error_turns() {
    let view = transcript_from(vec![
        assistant_text("good"),
        assistant_with_api_error("borked"),
    ]);
    let texts = collect_recent_assistant_texts(&view, 10);
    assert_eq!(texts, vec!["good"]);
}

#[test]
fn collect_skips_tool_only_turns_with_no_text_parts() {
    // Assistant message with only reasoning (no text) should be skipped.
    let no_text = assistant_with_parts(vec![AssistantContent::Reasoning(
        coco_messages::ReasoningContent {
            text: "internal thoughts".into(),
            provider_metadata: None,
        },
    )]);
    let view = transcript_from(vec![no_text, assistant_text("visible")]);
    let texts = collect_recent_assistant_texts(&view, 10);
    assert_eq!(texts, vec!["visible"]);
}

#[test]
fn collect_joins_multiple_text_parts_with_double_newline() {
    let multi = assistant_with_parts(vec![text_part("part one"), text_part("part two")]);
    let view = transcript_from(vec![multi]);
    let texts = collect_recent_assistant_texts(&view, 10);
    assert_eq!(texts, vec!["part one\n\npart two"]);
}

#[test]
fn collect_caps_at_max_lookback() {
    let messages: Vec<Message> = (0..MAX_LOOKBACK + 5)
        .map(|i| assistant_text(&format!("msg {i}")))
        .collect();
    let view = transcript_from(messages);
    let texts = collect_recent_assistant_texts(&view, MAX_LOOKBACK);
    assert_eq!(texts.len(), MAX_LOOKBACK);
    // Newest first: index 0 should be the last-pushed message.
    assert_eq!(texts[0], format!("msg {}", MAX_LOOKBACK + 4));
}

#[test]
fn collect_dedups_when_one_uuid_has_multiple_cells() {
    // Assistant with text + reasoning produces two cells from a single
    // message; collect should yield one text entry, not two.
    let msg = assistant_with_parts(vec![
        text_part("the visible answer"),
        AssistantContent::Reasoning(coco_messages::ReasoningContent {
            text: "hidden".into(),
            provider_metadata: None,
        }),
    ]);
    let view = transcript_from(vec![msg]);
    let texts = collect_recent_assistant_texts(&view, 10);
    assert_eq!(texts, vec!["the visible answer"]);
}

#[test]
fn collect_with_max_zero_returns_empty() {
    let view = transcript_from(vec![assistant_text("anything")]);
    assert!(collect_recent_assistant_texts(&view, 0).is_empty());
}
