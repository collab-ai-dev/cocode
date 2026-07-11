//! Tests for the leader inbox poller's teammate-message surfacing (gap 4b):
//! `format_idle_notification` framing and `enqueue_coordinator_message`
//! queue tagging. The full `poll_once` (mailbox→queue) needs a live
//! `SessionRuntime` + home-dir mailbox, so it's covered by the two-process
//! E2E rather than a unit test.

use super::*;

fn idle(
    from: &str,
    idle_reason: Option<&str>,
    summary: Option<&str>,
    completed_task_id: Option<&str>,
    completed_status: Option<&str>,
    failure_reason: Option<&str>,
) -> mailbox::ProtocolMessage {
    mailbox::ProtocolMessage::IdleNotification {
        from: from.to_string(),
        timestamp: "2026-06-04T00:00:00Z".to_string(),
        idle_reason: idle_reason.map(str::to_string),
        summary: summary.map(str::to_string),
        completed_task_id: completed_task_id.map(str::to_string),
        completed_status: completed_status.map(str::to_string),
        failure_reason: failure_reason.map(str::to_string),
    }
}

#[test]
fn format_idle_notification_attributes_teammate_and_task() {
    let msg = idle(
        "researcher",
        Some("available"),
        Some("found the bug"),
        Some("task-7"),
        Some("completed"),
        None,
    );
    let out = format_idle_notification(&msg);
    assert!(out.contains("teammate_id=\"researcher\""), "got: {out}");
    assert!(
        out.contains("is now idle and available (available)"),
        "got: {out}"
    );
    assert!(
        out.contains("completed task task-7 (completed)"),
        "got: {out}"
    );
    assert!(out.contains("summary=\"found the bug\""), "got: {out}");
}

#[test]
fn format_idle_notification_surfaces_failure() {
    let msg = idle(
        "builder",
        None,
        None,
        Some("task-1"),
        Some("failed"),
        Some("compile error"),
    );
    let out = format_idle_notification(&msg);
    assert!(out.contains("completed task task-1 (failed)"), "got: {out}");
    assert!(out.contains("failure: compile error"), "got: {out}");
}

#[test]
fn format_idle_notification_wrong_variant_is_empty() {
    let other = mailbox::ProtocolMessage::ModeSetRequest {
        mode: coco_types::PermissionMode::Plan,
        from: "team-lead".to_string(),
    };
    assert!(format_idle_notification(&other).is_empty());
}

#[tokio::test]
async fn enqueue_coordinator_message_tags_origin() {
    let queue = CommandQueue::new();
    let framed = "<teammate_message teammate_id=\"alice\">done</teammate_message>".to_string();
    enqueue_coordinator_message(&queue, framed.clone()).await;

    let cmd = queue.peek(None).await.expect("a queued command");
    assert_eq!(cmd.origin, Some(QueueOrigin::Coordinator));
    assert_eq!(cmd.prompt, framed);
    assert!(
        !cmd.is_slash_command,
        "teammate XML must not be parsed as a slash command"
    );
}

#[tokio::test]
async fn enqueue_coordinator_message_skips_empty() {
    let queue = CommandQueue::new();
    enqueue_coordinator_message(&queue, "   ".to_string()).await;
    assert!(queue.is_empty().await);
}
