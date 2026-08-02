use coco_messages::AssistantContent;
use coco_messages::MessageHistory;

use super::RecoveryWorkingContext;

fn assistant(text: &str) -> coco_messages::Message {
    coco_messages::create_assistant_message(
        vec![AssistantContent::text(text)],
        "test-model",
        coco_types::TokenUsage::default(),
    )
}

#[test]
fn compaction_drops_segments_whose_anchor_is_gone_and_keeps_the_rest_ordered() {
    let mut history = MessageHistory::new();
    let mut recovery = RecoveryWorkingContext::default();

    history.push(coco_messages::create_user_message("first durable"));
    recovery.append(
        &history,
        assistant("first malformed"),
        coco_messages::create_meta_message("first nudge"),
    );

    history.push(coco_messages::create_user_message("second durable"));
    recovery.append(
        &history,
        assistant("second malformed"),
        coco_messages::create_meta_message("second nudge"),
    );

    // Compaction replaced the earlier anchor. The first pair answered context
    // that no longer exists — relocating it would put a stale "you returned an
    // empty response" at the tail, pointing at nothing. The retained pair keeps
    // its position.
    let durable = vec![history.last().expect("second durable message").clone()];
    let assembled = recovery.assemble(&durable);
    let text: Vec<_> = assembled
        .iter()
        .map(|message| coco_messages::wrapping::extract_text_from_message(message))
        .collect();

    assert_eq!(text, ["second durable", "second malformed", "second nudge"]);
}

#[test]
fn request_context_precedes_recovery_but_durable_response_does_not() {
    let mut history = MessageHistory::new();
    let mut recovery = RecoveryWorkingContext::default();

    history.push(coco_messages::create_user_message("original request"));
    recovery.append(
        &history,
        assistant("malformed response"),
        coco_messages::create_meta_message("recovery nudge"),
    );

    let mut durable = history.to_vec();
    durable.push(std::sync::Arc::new(coco_messages::create_meta_message(
        "next request context",
    )));
    durable.push(std::sync::Arc::new(assistant("valid response")));

    let assembled = recovery.assemble(&durable);
    let text: Vec<_> = assembled
        .iter()
        .map(|message| coco_messages::wrapping::extract_text_from_message(message))
        .collect();

    assert_eq!(
        text,
        [
            "original request",
            "next request context",
            "malformed response",
            "recovery nudge",
            "valid response",
        ]
    );
}
