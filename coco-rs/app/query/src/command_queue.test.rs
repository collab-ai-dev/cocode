use super::*;

#[tokio::test]
async fn test_enqueue_dequeue_priority_order() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("later".into(), QueuePriority::Later))
        .await;
    queue
        .enqueue(QueuedCommand::new("now".into(), QueuePriority::Now))
        .await;
    queue
        .enqueue(QueuedCommand::new("next".into(), QueuePriority::Next))
        .await;

    let first = queue.dequeue(None).await.unwrap();
    assert_eq!(first.prompt, "now");
    let second = queue.dequeue(None).await.unwrap();
    assert_eq!(second.prompt, "next");
    let third = queue.dequeue(None).await.unwrap();
    assert_eq!(third.prompt, "later");
}

#[tokio::test]
async fn test_slash_commands_excluded_from_dequeue() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("/commit".into(), QueuePriority::Now))
        .await;
    queue
        .enqueue(QueuedCommand::new("fix bug".into(), QueuePriority::Next))
        .await;

    let cmd = queue.dequeue(None).await.unwrap();
    assert_eq!(cmd.prompt, "fix bug");
}

#[test]
fn test_new_classifies_whitespace_prefixed_slash_command() {
    let cmd = QueuedCommand::new("  /clear".into(), QueuePriority::Next);
    assert!(cmd.is_slash_command);
}

#[tokio::test]
async fn test_agent_id_filtering() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("main".into(), QueuePriority::Next))
        .await;
    queue
        .enqueue(QueuedCommand::new("agent1".into(), QueuePriority::Next).with_agent("a1".into()))
        .await;

    // Only get agent1's commands.
    let cmd = queue.dequeue(Some("a1")).await.unwrap();
    assert_eq!(cmd.prompt, "agent1");

    // Main thread gets its own.
    let cmd = queue.dequeue(None).await.unwrap();
    assert_eq!(cmd.prompt, "main");
}

#[tokio::test]
async fn test_get_commands_by_max_priority() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("urgent".into(), QueuePriority::Now))
        .await;
    queue
        .enqueue(QueuedCommand::new("normal".into(), QueuePriority::Next))
        .await;
    queue
        .enqueue(QueuedCommand::new("bg".into(), QueuePriority::Later))
        .await;

    let up_to_next = queue
        .get_commands_by_max_priority(QueuePriority::Next, None)
        .await;
    assert_eq!(up_to_next.len(), 2);
}

#[tokio::test]
async fn test_remove_commands() {
    let queue = CommandQueue::new();
    let keep = QueuedCommand::new("keep".into(), QueuePriority::Next);
    let drop = QueuedCommand::new("remove".into(), QueuePriority::Next);
    let drop_id = drop.id;
    queue.enqueue(keep).await;
    queue.enqueue(drop).await;

    queue.remove_by_ids(&[drop_id]).await;
    assert_eq!(queue.len().await, 1);
}

#[tokio::test]
async fn test_remove_by_id_returns_removed_command() {
    let queue = CommandQueue::new();
    let keep = QueuedCommand::new("keep".into(), QueuePriority::Next);
    let edit = QueuedCommand::new("edit me".into(), QueuePriority::Next);
    let edit_id = edit.id;
    queue.enqueue(keep).await;
    queue.enqueue(edit).await;

    let removed = queue
        .remove_by_id(edit_id)
        .await
        .expect("queued command removed");

    assert_eq!(removed.prompt, "edit me");
    assert_eq!(queue.len().await, 1);
    assert!(queue.remove_by_id(edit_id).await.is_none());
}

#[tokio::test]
async fn test_peek_does_not_remove() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("hello".into(), QueuePriority::Next))
        .await;

    let peeked = queue.peek(None).await;
    assert_eq!(peeked.unwrap().prompt, "hello");
    // Still in queue.
    assert_eq!(queue.len().await, 1);
}

#[tokio::test]
async fn test_dequeue_matching() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("keep".into(), QueuePriority::Next))
        .await;
    queue
        .enqueue(QueuedCommand::new("remove-1".into(), QueuePriority::Next))
        .await;
    queue
        .enqueue(QueuedCommand::new("remove-2".into(), QueuePriority::Later))
        .await;

    let removed = queue
        .dequeue_matching(|c| c.prompt.starts_with("remove"))
        .await;
    assert_eq!(removed.len(), 2);
    assert_eq!(queue.len().await, 1);
}

#[tokio::test]
async fn test_dequeue_all_editable_keeps_non_user_queue_items() {
    let queue = CommandQueue::new();
    queue
        .enqueue(
            QueuedCommand::new("human".into(), QueuePriority::Next)
                .with_origin(coco_system_reminder::QueueOrigin::Human),
        )
        .await;
    queue
        .enqueue(QueuedCommand::new(
            "legacy-human".into(),
            QueuePriority::Next,
        ))
        .await;
    queue
        .enqueue(
            QueuedCommand::new("task".into(), QueuePriority::Later)
                .with_origin(coco_system_reminder::QueueOrigin::TaskNotification),
        )
        .await;
    queue
        .enqueue(QueuedCommand::new("/compact".into(), QueuePriority::Next))
        .await;
    queue
        .enqueue(QueuedCommand::new("agent".into(), QueuePriority::Next).with_agent("a1".into()))
        .await;

    let editable = queue.dequeue_all_editable().await;

    let prompts: Vec<&str> = editable.iter().map(|cmd| cmd.prompt.as_str()).collect();
    assert_eq!(prompts, vec!["human", "legacy-human"]);
    assert_eq!(queue.len().await, 3);
}

#[tokio::test]
async fn test_dequeue_first_matching_keeps_later_matches() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new(
            "/compact one".into(),
            QueuePriority::Next,
        ))
        .await;
    queue
        .enqueue(QueuedCommand::new(
            "/compact two".into(),
            QueuePriority::Next,
        ))
        .await;

    let removed = queue
        .dequeue_first_matching(|c| c.is_slash_command)
        .await
        .expect("first slash command should be removed");
    assert_eq!(removed.prompt, "/compact one");
    assert_eq!(queue.len().await, 1);
    let remaining = queue
        .dequeue_first_matching(|c| c.is_slash_command)
        .await
        .expect("second slash command should remain");
    assert_eq!(remaining.prompt, "/compact two");
}

#[tokio::test]
async fn test_clear() {
    let queue = CommandQueue::new();
    queue
        .enqueue(QueuedCommand::new("a".into(), QueuePriority::Now))
        .await;
    queue
        .enqueue(QueuedCommand::new("b".into(), QueuePriority::Next))
        .await;

    queue.clear().await;
    assert!(queue.is_empty().await);
}
