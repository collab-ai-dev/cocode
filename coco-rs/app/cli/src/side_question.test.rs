use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::Message;
use coco_messages::create_assistant_message;
use pretty_assertions::assert_eq;

use super::extract_side_question_answer;

fn assistant(text: &str) -> Arc<Message> {
    Arc::new(create_assistant_message(
        vec![AssistantContent::text(text)],
        "test-model",
        Default::default(),
    ))
}

#[test]
fn extract_joins_text_across_per_block_messages() {
    // The provider yields one assistant message per content block, so the
    // answer must concatenate text across ALL of them — not just the last
    // (the old single-message walk dropped earlier blocks).
    let msgs = vec![assistant("part one"), assistant("part two")];
    assert_eq!(extract_side_question_answer(&msgs), "part one\n\npart two");
}

#[test]
fn extract_skips_empty_text_messages() {
    // A leading thinking-only message extracts to empty text and must be
    // skipped rather than short-circuiting to "no response".
    let msgs = vec![assistant(""), assistant("real answer")];
    assert_eq!(extract_side_question_answer(&msgs), "real answer");
}

#[test]
fn extract_no_assistant_content_returns_no_response() {
    assert_eq!(extract_side_question_answer(&[]), "(No response received.)");
}
