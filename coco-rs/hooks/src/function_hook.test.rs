use std::sync::Arc;
use std::time::Duration;

use coco_messages::Message;
use coco_types::HookEventType;

use super::FunctionHook;
use super::FunctionHookPredicate;

#[derive(Debug)]
struct AlwaysPasses;

impl FunctionHookPredicate for AlwaysPasses {
    fn evaluate(&self, _messages: &[Arc<Message>]) -> bool {
        true
    }
    fn name(&self) -> &str {
        "AlwaysPasses"
    }
}

#[derive(Debug)]
struct CountsAtLeast(usize);

impl FunctionHookPredicate for CountsAtLeast {
    fn evaluate(&self, messages: &[Arc<Message>]) -> bool {
        messages.len() >= self.0
    }
    fn name(&self) -> &str {
        "CountsAtLeast"
    }
}

#[test]
fn predicate_name_is_observable_for_logging() {
    let p: Arc<dyn FunctionHookPredicate> = Arc::new(AlwaysPasses);
    assert_eq!(p.name(), "AlwaysPasses");
}

#[test]
fn predicate_evaluates_against_history_slice() {
    let p = CountsAtLeast(2);
    assert!(!p.evaluate(&[]));
    assert!(!p.evaluate(&[Arc::new(make_user_msg("a"))]));
    assert!(p.evaluate(&[Arc::new(make_user_msg("a")), Arc::new(make_user_msg("b"))]));
}

#[test]
fn function_hook_debug_renders_without_callback_internals() {
    let hook = FunctionHook {
        id: "test-1".into(),
        event: HookEventType::Stop,
        matcher: None,
        timeout: Duration::from_secs(5),
        predicate: Arc::new(AlwaysPasses),
        error_message: "must call X".into(),
    };
    let rendered = format!("{hook:?}");
    assert!(rendered.contains("FunctionHook"));
    assert!(rendered.contains("test-1"));
    assert!(rendered.contains("AlwaysPasses"));
    // Timeout renders as Duration; we just confirm the field shows.
    assert!(rendered.contains("timeout"));
}

#[test]
fn function_hook_is_clone() {
    let hook = FunctionHook {
        id: "test-2".into(),
        event: HookEventType::Stop,
        matcher: Some("any".into()),
        timeout: Duration::from_millis(500),
        predicate: Arc::new(AlwaysPasses),
        error_message: "msg".into(),
    };
    let cloned = hook.clone();
    assert_eq!(cloned.id, "test-2");
    assert_eq!(cloned.matcher.as_deref(), Some("any"));
    // Arc-share check: same pointer.
    assert!(Arc::ptr_eq(&cloned.predicate, &hook.predicate));
}

fn make_user_msg(text: &str) -> Message {
    Message::User(coco_messages::UserMessage {
        message: coco_messages::LlmMessage::user_text(text.to_string()),
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
