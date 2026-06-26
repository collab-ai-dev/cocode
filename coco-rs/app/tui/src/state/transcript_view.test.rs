use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::Message;
use coco_messages::TextContent;
use coco_messages::create_assistant_message;
use coco_messages::create_user_message_with_uuid;
use coco_types::TokenUsage;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use super::TranscriptView;

#[test]
fn revision_increments_on_visible_mutations_and_skips_duplicate_uuid_noop() {
    let mut view = TranscriptView::new();
    let first = Uuid::new_v4();

    view.on_message_appended(Arc::new(create_user_message_with_uuid(first, "hello")));
    assert_eq!(view.revision(), 1);

    view.on_message_appended(Arc::new(create_user_message_with_uuid(first, "duplicate")));
    assert_eq!(view.revision(), 1);

    let second = create_assistant_message(
        vec![AssistantContent::Text(TextContent::new("world"))],
        "test-model",
        TokenUsage::default(),
    );
    view.on_message_appended(Arc::new(second));
    assert_eq!(view.revision(), 2);

    view.on_message_truncated(1);
    assert_eq!(view.revision(), 3);

    view.on_session_reset();
    assert_eq!(view.revision(), 4);

    view.replace_from_messages(&[Arc::new(create_user_message_with_uuid(
        Uuid::new_v4(),
        "replacement",
    ))]);
    assert_eq!(view.revision(), 5);
}

#[test]
fn transcript_projection_is_a_pure_function_of_the_persisted_log() {
    // opencode's `replaySessionProjection` property: the derived read-model
    // must be a pure function of the persisted message log. We persist through
    // the message serde codec (the same codec the JSONL transcript store uses),
    // reload, and assert the projected transcript is byte-identical — the
    // correctness basis for resume/recovery.
    let original: Vec<Arc<Message>> = vec![
        Arc::new(create_user_message_with_uuid(
            Uuid::new_v4(),
            "first prompt",
        )),
        Arc::new(create_assistant_message(
            vec![AssistantContent::Text(TextContent::new("a reply"))],
            "test-model",
            TokenUsage::default(),
        )),
        Arc::new(create_user_message_with_uuid(
            Uuid::new_v4(),
            "second prompt",
        )),
    ];

    // Persist → reload: one JSON value per message (the JSONL transcript shape).
    let reloaded: Vec<Arc<Message>> = original
        .iter()
        .map(|m| {
            let json = serde_json::to_string(m.as_ref()).expect("message serializes");
            Arc::new(serde_json::from_str::<Message>(&json).expect("message round-trips"))
        })
        .collect();

    // Project both logs. `RenderedCell` has no `PartialEq`, so compare a
    // deterministic projection: (uuid, CellKind debug). `CellKind` holds only
    // simple scalar fields (no maps), so its `Debug` is order-stable.
    let project = |msgs: &[Arc<Message>]| {
        let mut view = TranscriptView::new();
        view.replace_from_messages(msgs);
        view.cells_for_test()
            .iter()
            .map(|c| (c.message_uuid, format!("{:?}", c.kind)))
            .collect::<Vec<_>>()
    };

    let from_original = project(&original);
    let from_reloaded = project(&reloaded);

    assert!(!from_original.is_empty(), "projection produced no cells");
    assert_eq!(
        from_original, from_reloaded,
        "derived transcript must be identical after persist→reload \
         (read-model is a pure function of the log)"
    );
}
