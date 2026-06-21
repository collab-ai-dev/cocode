use std::sync::Arc;

use coco_messages::create_user_message;
use pretty_assertions::assert_eq;

use super::LiveTranscript;

#[test]
fn new_handle_snapshots_empty() {
    let lt = LiveTranscript::new();
    assert!(lt.snapshot().is_empty());
}

#[test]
fn set_then_snapshot_returns_latest() {
    let lt = LiveTranscript::new();
    lt.set(vec![Arc::new(create_user_message("first"))]);
    assert_eq!(lt.snapshot().len(), 1);

    // Full-snapshot replace: a second `set` overwrites, it does not append.
    lt.set(vec![
        Arc::new(create_user_message("a")),
        Arc::new(create_user_message("b")),
    ]);
    assert_eq!(lt.snapshot().len(), 2);
}

#[test]
fn clone_shares_the_same_buffer() {
    let writer = LiveTranscript::new();
    let reader = writer.clone();
    writer.set(vec![Arc::new(create_user_message("hello"))]);
    // Reader observes the writer's update through the shared `Arc`.
    assert_eq!(reader.snapshot().len(), 1);
}
