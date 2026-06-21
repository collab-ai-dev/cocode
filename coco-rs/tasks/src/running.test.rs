use coco_types::TaskStatus;
use coco_types::TaskType;

use super::*;

async fn create_pending(
    mgr: &TaskManager,
    task_type: TaskType,
    description: &str,
    output_file: &str,
) -> String {
    create_with_status(
        mgr,
        task_type,
        description,
        output_file,
        TaskStatus::Pending,
    )
    .await
}

async fn create_running(
    mgr: &TaskManager,
    task_type: TaskType,
    description: &str,
    output_file: &str,
) -> String {
    create_with_status(
        mgr,
        task_type,
        description,
        output_file,
        TaskStatus::Running,
    )
    .await
}

async fn create_with_status(
    mgr: &TaskManager,
    task_type: TaskType,
    description: &str,
    output_file: &str,
    status: TaskStatus,
) -> String {
    let id = coco_types::generate_task_id(task_type);
    mgr.create_task(TaskCreateRequest {
        task_id: id,
        task_type,
        description: description.to_string(),
        output_file: Some(output_file.to_string()),
        tool_use_id: None,
        is_backgrounded: false,
        status,
        cancel: tokio_util::sync::CancellationToken::new(),
        invoking_agent: None,
        shell_extras: None,
    })
    .await
}

#[tokio::test]
async fn test_task_create_and_get() {
    let mgr = TaskManager::new();
    let id = create_pending(&mgr, TaskType::Shell, "run tests", "/tmp/output.txt").await;

    assert!(id.starts_with('b')); // LocalBash prefix

    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.description, "run tests");
}

#[tokio::test]
async fn local_agent_task_id_matches_ts_agent_id_shape() {
    let id = coco_types::generate_task_id(TaskType::BgAgent);

    assert_eq!(id.len(), 17);
    assert!(id.starts_with('a'));
    assert!(id[1..].chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn test_task_update_status() {
    let mgr = TaskManager::new();
    let id = create_pending(&mgr, TaskType::BgAgent, "agent work", "/tmp/out.txt").await;

    mgr.update_status(&id, TaskStatus::Running).await;
    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Running);

    mgr.update_status(&id, TaskStatus::Completed).await;
    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.end_time.is_some());
}

#[tokio::test]
async fn test_task_list() {
    let mgr = TaskManager::new();
    create_pending(&mgr, TaskType::Shell, "t1", "/tmp/1").await;
    create_pending(&mgr, TaskType::Shell, "t2", "/tmp/2").await;
    create_pending(&mgr, TaskType::BgAgent, "t3", "/tmp/3").await;

    let list = mgr.list().await;
    assert_eq!(list.len(), 3);
}

#[tokio::test]
async fn test_kill_task() {
    let mgr = TaskManager::new();
    let id = create_pending(&mgr, TaskType::Shell, "long running", "/tmp/out.txt").await;

    mgr.update_status(&id, TaskStatus::Running).await;
    mgr.update_status(&id, TaskStatus::Killed).await;

    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Killed);
    assert!(task.end_time.is_some());
}

#[tokio::test]
async fn test_task_lifecycle() {
    let mgr = TaskManager::new();
    let id = create_pending(&mgr, TaskType::BgAgent, "build project", "/tmp/build.txt").await;

    // Pending -> Running -> Completed
    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.end_time.is_none());

    mgr.update_status(&id, TaskStatus::Running).await;
    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Running);
    assert!(task.end_time.is_none());

    mgr.update_status(&id, TaskStatus::Completed).await;
    let task = mgr.get(&id).await.expect("task should exist");
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.end_time.is_some());
    assert!(task.status.is_terminal());
}

#[tokio::test]
async fn test_remove_completed() {
    let mgr = TaskManager::new();
    let id1 = create_pending(&mgr, TaskType::Shell, "done", "/tmp/1.txt").await;
    let id2 = create_pending(&mgr, TaskType::Shell, "still running", "/tmp/2.txt").await;
    let id3 = create_pending(&mgr, TaskType::Shell, "failed", "/tmp/3.txt").await;

    mgr.update_status(&id1, TaskStatus::Completed).await;
    mgr.update_status(&id2, TaskStatus::Running).await;
    mgr.update_status(&id3, TaskStatus::Failed).await;
    mgr.mark_notified_once(&id1).await;
    mgr.mark_notified_once(&id3).await;

    let removed = mgr.remove_completed().await;
    assert_eq!(removed, 2); // id1 (Completed) + id3 (Failed)

    // Only the running task remains
    let remaining = mgr.list().await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, id2);
}

#[tokio::test]
async fn remove_completed_keeps_unnotified_terminal_tasks() {
    let mgr = TaskManager::new();
    let id = create_running(&mgr, TaskType::Shell, "done", "/tmp/1.txt").await;

    mgr.transition_terminal(&id, TaskStatus::Completed).await;
    assert_eq!(mgr.remove_completed().await, 0);
    assert!(mgr.get(&id).await.is_some());

    mgr.mark_notified_once(&id).await;
    assert_eq!(mgr.remove_completed().await, 1);
    assert!(mgr.get(&id).await.is_none());
}

// ─── WS-6: event sink emission ─────────────────────────────────────────
//
// When constructed with `with_event_sink(tx)`, TaskManager emits the
// matching `ServerNotification::TaskStarted/TaskProgress/TaskCompleted`
// for every lifecycle transition. Tests exercise the full round-trip
// for one happy-path task plus one failure path.

use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TaskCompletionStatus;
use tokio::sync::mpsc;

fn collect(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<ServerNotification> {
    let mut out = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let CoreEvent::Protocol(n) = evt {
            out.push(n);
        }
    }
    out
}

#[tokio::test]
async fn test_event_sink_happy_path() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let mgr = TaskManager::new().with_event_sink(tx);

    let id = create_pending(&mgr, TaskType::BgAgent, "build project", "/tmp/build.txt").await;
    mgr.update_status(&id, TaskStatus::Running).await;
    mgr.update_status(&id, TaskStatus::Completed).await;

    let events = collect(&mut rx);
    assert_eq!(events.len(), 3, "expected 3 events, got: {events:?}");

    match &events[0] {
        ServerNotification::TaskStarted(p) => {
            assert_eq!(p.task_id, id);
            assert_eq!(p.description, "build project");
            assert_eq!(p.task_type.as_deref(), Some("local_agent"));
        }
        other => panic!("expected TaskStarted, got {other:?}"),
    }
    match &events[1] {
        ServerNotification::TaskProgress(p) => {
            assert_eq!(p.task_id, id);
            assert_eq!(p.description, "build project");
        }
        other => panic!("expected TaskProgress, got {other:?}"),
    }
    match &events[2] {
        ServerNotification::TaskCompleted(p) => {
            assert_eq!(p.task_id, id);
            assert_eq!(p.status, TaskCompletionStatus::Completed);
            assert_eq!(p.output_file, "/tmp/build.txt");
        }
        other => panic!("expected TaskCompleted, got {other:?}"),
    }
}

#[tokio::test]
async fn create_task_emits_started_with_tool_use_id() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let mgr = TaskManager::new().with_event_sink(tx);
    let id = coco_types::generate_task_id(TaskType::BgAgent);

    mgr.create_task(TaskCreateRequest {
        task_id: id.clone(),
        task_type: TaskType::BgAgent,
        description: "agent work".to_string(),
        output_file: Some("/tmp/agent.txt".to_string()),
        tool_use_id: Some("toolu_123".to_string()),
        is_backgrounded: false,
        status: TaskStatus::Running,
        cancel: tokio_util::sync::CancellationToken::new(),
        invoking_agent: None,
        shell_extras: None,
    })
    .await;

    let events = collect(&mut rx);
    match &events[0] {
        ServerNotification::TaskStarted(p) => {
            assert_eq!(p.task_id, id);
            assert_eq!(p.tool_use_id.as_deref(), Some("toolu_123"));
        }
        other => panic!("expected TaskStarted, got {other:?}"),
    }
}

#[tokio::test]
async fn mark_notified_once_suppresses_duplicates() {
    let mgr = TaskManager::new();
    let id = create_running(&mgr, TaskType::BgAgent, "agent", "/tmp/out").await;

    assert!(mgr.mark_notified_once(&id).await);
    assert!(!mgr.mark_notified_once(&id).await);
    let state = mgr.get(&id).await.unwrap();
    assert!(state.notified);
}

#[tokio::test]
async fn kill_running_rejects_terminal_and_removed_tasks() {
    let mgr = TaskManager::new();
    let id = create_running(&mgr, TaskType::Shell, "shell", "/tmp/out").await;
    mgr.transition_terminal(&id, TaskStatus::Completed).await;

    assert_eq!(mgr.kill_running(&id).await, Err(KillTaskError::NotRunning));
    assert!(mgr.remove_task(&id).await);
    assert_eq!(mgr.kill_running(&id).await, Err(KillTaskError::NotFound));
}

#[tokio::test]
async fn kill_running_marks_shell_notified_to_suppress_stop_noise() {
    let mgr = TaskManager::new();
    let id = create_running(&mgr, TaskType::Shell, "shell", "/tmp/out").await;

    mgr.kill_running(&id).await.unwrap();

    let state = mgr.get(&id).await.unwrap();
    assert!(state.notified);
    assert_eq!(state.status, TaskStatus::Running);
}

#[tokio::test]
async fn transition_terminal_marks_dream_notified() {
    let mgr = TaskManager::new();
    let id = create_running(&mgr, TaskType::Dream, "dream", "/tmp/out").await;

    mgr.transition_terminal(&id, TaskStatus::Completed).await;

    let state = mgr.get(&id).await.unwrap();
    assert!(state.notified);
}

#[tokio::test]
async fn test_event_sink_failure_maps_to_failed_status() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let mgr = TaskManager::new().with_event_sink(tx);

    let id = create_pending(&mgr, TaskType::Shell, "flaky script", "/tmp/out.txt").await;
    mgr.update_status(&id, TaskStatus::Failed).await;

    let events = collect(&mut rx);
    assert_eq!(events.len(), 2);
    match &events[1] {
        ServerNotification::TaskCompleted(p) => {
            assert_eq!(p.status, TaskCompletionStatus::Failed);
        }
        other => panic!("expected TaskCompleted(Failed), got {other:?}"),
    }
}

#[tokio::test]
async fn test_event_sink_killed_maps_to_stopped_status() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let mgr = TaskManager::new().with_event_sink(tx);

    let id = create_pending(&mgr, TaskType::Shell, "long job", "/tmp/out.txt").await;
    mgr.update_status(&id, TaskStatus::Killed).await;

    let events = collect(&mut rx);
    assert_eq!(events.len(), 2);
    match &events[1] {
        ServerNotification::TaskCompleted(p) => {
            assert_eq!(p.status, TaskCompletionStatus::Stopped);
        }
        other => panic!("expected TaskCompleted(Stopped), got {other:?}"),
    }
}

#[tokio::test]
async fn test_no_emission_without_sink() {
    // Default (no sink) must not panic and must not emit anything.
    let mgr = TaskManager::new();
    let id = create_pending(&mgr, TaskType::Shell, "no sink", "/tmp/out.txt").await;
    mgr.update_status(&id, TaskStatus::Running).await;
    mgr.update_status(&id, TaskStatus::Completed).await;
    // Nothing observable to assert — just that these calls don't panic
    // and the task state updates normally.
    let task = mgr.get(&id).await.expect("task exists");
    assert_eq!(task.status, TaskStatus::Completed);
}

#[tokio::test]
async fn teammate_task_creation_emits_ts_wire_type() {
    let (tx, mut rx) = mpsc::channel::<CoreEvent>(16);
    let mgr = TaskManager::new().with_event_sink(tx);
    let id = coco_types::generate_task_id(TaskType::Teammate);

    mgr.create_teammate_task(TeammateTaskCreateRequest {
        task_id: id.clone(),
        agent_ref: coco_types::TeammateRef::new("worker", "test"),
        backend_type: coco_types::BackendType::InProcess,
        pane_id: None,
        prompt: "do work".to_string(),
        output_file: Some("/tmp/worker.out".to_string()),
        cancel: tokio_util::sync::CancellationToken::new(),
    })
    .await;

    let events = collect(&mut rx);
    match &events[0] {
        ServerNotification::TaskStarted(p) => {
            assert_eq!(p.task_id, id);
            assert_eq!(p.task_type.as_deref(), Some("in_process_teammate"));
        }
        other => panic!("expected TaskStarted, got {other:?}"),
    }
}

#[tokio::test]
async fn find_teammate_by_agent_id_prefers_running_row() {
    let mgr = TaskManager::new();
    let agent_id = "worker@test";
    let old_id = coco_types::generate_task_id(TaskType::Teammate);
    mgr.create_teammate_task(TeammateTaskCreateRequest {
        task_id: old_id.clone(),
        agent_ref: coco_types::TeammateRef::new("worker", "test"),
        backend_type: coco_types::BackendType::InProcess,
        pane_id: None,
        prompt: "old".to_string(),
        output_file: Some("/tmp/old.out".to_string()),
        cancel: tokio_util::sync::CancellationToken::new(),
    })
    .await;
    mgr.transition_terminal(&old_id, TaskStatus::Completed)
        .await;

    let running_id = coco_types::generate_task_id(TaskType::Teammate);
    mgr.create_teammate_task(TeammateTaskCreateRequest {
        task_id: running_id.clone(),
        agent_ref: coco_types::TeammateRef::new("worker", "test"),
        backend_type: coco_types::BackendType::InProcess,
        pane_id: None,
        prompt: "new".to_string(),
        output_file: Some("/tmp/new.out".to_string()),
        cancel: tokio_util::sync::CancellationToken::new(),
    })
    .await;

    let found = mgr.find_teammate(agent_id).await.unwrap();
    assert_eq!(found.id, running_id);
}

#[tokio::test]
async fn teammate_current_work_interrupt_cancels_only_active_turn() {
    let mgr = TaskManager::new();
    let id = coco_types::generate_task_id(TaskType::Teammate);
    let lifecycle = tokio_util::sync::CancellationToken::new();
    mgr.create_teammate_task(TeammateTaskCreateRequest {
        task_id: id,
        agent_ref: coco_types::TeammateRef::new("worker", "test"),
        backend_type: coco_types::BackendType::InProcess,
        pane_id: None,
        prompt: "work".to_string(),
        output_file: Some("/tmp/worker.out".to_string()),
        cancel: lifecycle.clone(),
    })
    .await;
    let turn = tokio_util::sync::CancellationToken::new();
    let observed_turn = turn.clone();
    assert!(
        mgr.set_teammate_current_work_cancel("worker@test", Some(turn))
            .await
    );

    assert!(
        mgr.interrupt_teammate_current_work("worker@test")
            .await
            .unwrap()
    );
    assert!(observed_turn.is_cancelled());
    assert!(!lifecycle.is_cancelled());

    assert!(
        mgr.set_teammate_current_work_cancel("worker@test", None)
            .await
    );
    assert!(
        !mgr.interrupt_teammate_current_work("worker@test")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn remove_completed_removes_teammate_row_and_control_after_notification() {
    let mgr = TaskManager::new();
    let id = coco_types::generate_task_id(TaskType::Teammate);
    mgr.create_teammate_task(TeammateTaskCreateRequest {
        task_id: id.clone(),
        agent_ref: coco_types::TeammateRef::new("worker", "test"),
        backend_type: coco_types::BackendType::Tmux,
        pane_id: Some("%1".to_string()),
        prompt: "work".to_string(),
        output_file: Some("/tmp/worker.out".to_string()),
        cancel: tokio_util::sync::CancellationToken::new(),
    })
    .await;
    mgr.transition_terminal(&id, TaskStatus::Killed).await;
    assert_eq!(mgr.remove_completed().await, 0);
    assert!(mgr.mark_notified_once(&id).await);

    assert_eq!(mgr.remove_completed().await, 1);
    assert!(mgr.get(&id).await.is_none());
    assert_eq!(mgr.kill_running(&id).await, Err(KillTaskError::NotFound));
}
