use super::*;
use coco_hooks::AsyncRewakeSink;
use coco_query::command_queue::{CommandQueue, QueuePriority};
use coco_tasks::{NotificationKind, TerminalStatus};

fn shell_terminal(task_id: &str, agent_id: Option<&str>) -> TaskNotification {
    TaskNotification {
        task_id: task_id.into(),
        tool_use_id: None,
        agent_id: agent_id.map(String::from),
        output_file: "/tmp/out".into(),
        description: "ls".into(),
        kind: NotificationKind::ShellTerminal {
            status: TerminalStatus::Completed,
            exit_code: Some(0),
            killed_by: None,
        },
    }
}

#[tokio::test]
async fn push_terminal_uses_later_priority() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(shell_terminal("a", None)).await;
    let drained = q
        .dequeue_matching(|c| !c.is_slash_command && c.agent_id.is_none())
        .await;
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].priority, QueuePriority::Later);
}

#[tokio::test]
async fn push_stall_uses_next_priority() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(TaskNotification {
        task_id: "b".into(),
        tool_use_id: None,
        agent_id: None,
        output_file: "/tmp/out".into(),
        description: "x".into(),
        kind: NotificationKind::Stall {
            output_tail: "Continue?".into(),
        },
    })
    .await;
    let drained = q
        .dequeue_matching(|c| !c.is_slash_command && c.agent_id.is_none())
        .await;
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].priority, QueuePriority::Next);
    assert!(!drained[0].prompt.contains("<status>"));
}

#[tokio::test]
async fn push_with_agent_id_routes_to_agent_queue() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(shell_terminal("a", Some("agent-42"))).await;
    // The agent-routed item shouldn't show up in the main-thread filter.
    assert!(q.dequeue(None).await.is_none());
    let drained = q.dequeue(Some("agent-42")).await.expect("agent item");
    assert_eq!(drained.agent_id.as_deref(), Some("agent-42"));
}

#[tokio::test]
async fn push_tags_origin_as_task_notification() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(shell_terminal("a", None)).await;
    let snapshot = q.snapshot_for_reminder(None).await;
    assert_eq!(snapshot.len(), 1);
    assert!(matches!(
        snapshot[0].origin,
        Some(QueueOrigin::TaskNotification)
    ));
}

#[tokio::test]
async fn push_carries_typed_task_notification_payload() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(shell_terminal("a", None)).await;
    let cmd = q.dequeue(None).await.expect("queued command");
    let payload = cmd
        .task_notification
        .as_ref()
        .expect("typed task notification payload");
    assert_eq!(payload.task_id, "a");
    assert_eq!(
        payload.summary,
        "Background command \"ls\" completed (exit code 0)"
    );
    assert_eq!(payload.status, Some(coco_types::TaskStatus::Completed));
    assert_eq!(
        payload.source,
        coco_types::TaskNotificationSource::ShellTerminal
    );
    assert_eq!(payload.output_file.as_deref(), Some("/tmp/out"));
}

#[tokio::test]
async fn async_rewake_carries_hook_rewake_payload() {
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.enqueue_rewake("hook-name".into(), "rewake now".into())
        .await;
    let cmd = q.dequeue(None).await.expect("queued rewake");
    let payload = cmd
        .task_notification
        .as_ref()
        .expect("typed rewake payload");
    assert_eq!(payload.task_id, "hook-name");
    assert_eq!(payload.summary, "rewake now");
    assert_eq!(payload.status, None);
    assert_eq!(
        payload.source,
        coco_types::TaskNotificationSource::HookRewake
    );
    assert_eq!(payload.output_file, None);
}

#[tokio::test]
async fn agent_terminal_envelope_includes_result_usage_worktree() {
    use coco_tasks::{TaskUsage, Worktree};
    let q = CommandQueue::new();
    let sink = CommandQueueNotificationSink::new(q.clone());
    sink.push(TaskNotification {
        task_id: "ta1".into(),
        tool_use_id: Some("toolu_a".into()),
        agent_id: None,
        output_file: "/tmp/ta1.out".into(),
        description: "explore".into(),
        kind: NotificationKind::AgentTerminal {
            status: TerminalStatus::Completed,
            result: Some("found 3 callers".into()),
            usage: Some(TaskUsage {
                total_tokens: 1000,
                tool_uses: 4,
                duration_ms: 8000,
            }),
            worktree: Some(Worktree {
                path: "/tmp/wt".into(),
                branch: Some("feat/x".into()),
            }),
            error: None,
            killed_by: None,
        },
    })
    .await;
    let drained = q
        .dequeue_matching(|c| !c.is_slash_command && c.agent_id.is_none())
        .await;
    assert_eq!(drained.len(), 1);
    let xml = &drained[0].prompt;
    assert!(xml.contains("<result>found 3 callers</result>"));
    assert!(xml.contains(
        "<usage><total_tokens>1000</total_tokens><tool_uses>4</tool_uses><duration_ms>8000</duration_ms></usage>"
    ));
    assert!(xml.contains("<worktreePath>/tmp/wt</worktreePath>"));
    assert!(xml.contains("<worktreeBranch>feat/x</worktreeBranch>"));
}
