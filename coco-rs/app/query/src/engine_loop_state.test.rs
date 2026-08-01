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
fn compaction_of_an_earlier_anchor_preserves_recovery_segment_order() {
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

    // Simulate compaction replacing only the earlier anchor. The retained
    // later anchor must not let the second segment leapfrog the first.
    let durable = vec![history.last().expect("second durable message").clone()];
    let assembled = recovery.assemble(&durable);
    let text: Vec<_> = assembled
        .iter()
        .map(|message| coco_messages::wrapping::extract_text_from_message(message))
        .collect();

    assert_eq!(
        text,
        [
            "second durable",
            "first malformed",
            "first nudge",
            "second malformed",
            "second nudge",
        ]
    );
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
