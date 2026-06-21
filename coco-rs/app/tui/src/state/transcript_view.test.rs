use std::sync::Arc;

use coco_messages::AssistantContent;
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
