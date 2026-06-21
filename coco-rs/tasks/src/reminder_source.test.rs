use super::*;
use crate::running::TaskManager;
use coco_system_reminder::TaskStatusSource;
use coco_types::TaskStatus;
use coco_types::TaskType;

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
    mgr.create_task(crate::running::TaskCreateRequest {
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
async fn collect_returns_empty_when_not_post_compact() {
    let mgr = TaskManager::new();
    create_pending(&mgr, TaskType::BgAgent, "foo", "/tmp/foo.log").await;
    let out = mgr.collect(None, /*just_compacted=*/ false).await;
    assert!(out.is_empty());
}

#[tokio::test]
async fn collect_emits_snapshot_post_compact_for_running() {
    let mgr = TaskManager::new();
    let _id = create_running(&mgr, TaskType::BgAgent, "scan repo", "/tmp/scan.log").await;
    let out = mgr.collect(None, true).await;
    assert_eq!(out.len(), 1);
    let s = &out[0];
    assert_eq!(s.description, "scan repo");
    assert_eq!(s.output_file_path.as_deref(), Some("/tmp/scan.log"));
    assert!(
        matches!(s.status, coco_system_reminder::TaskRunStatus::Running),
        "running task survives the filter as TaskRunStatus::Running"
    );
}

/// The post-compact reminder MUST include terminal LocalAgent tasks
/// (Completed / Failed / Killed) whose `<task-notification>` envelope
/// was wiped from the CommandQueue by compaction. The render path at
/// `coco_system_reminder::generators::task_status::render_one` dispatches
/// per-status — running, killed, completed/failed all produce model-
/// visible text.
#[tokio::test]
async fn collect_includes_terminal_tasks_post_compact() {
    let mgr = TaskManager::new();
    let id_completed = create_running(&mgr, TaskType::BgAgent, "done", "/tmp/done.log").await;
    let id_failed = create_running(&mgr, TaskType::BgAgent, "broke", "/tmp/broke.log").await;
    let id_killed = create_running(&mgr, TaskType::BgAgent, "stopped", "/tmp/stopped.log").await;
    let id_running = create_running(&mgr, TaskType::BgAgent, "alive", "/tmp/alive.log").await;

    mgr.update_status(&id_completed, TaskStatus::Completed)
        .await;
    mgr.update_status(&id_failed, TaskStatus::Failed).await;
    mgr.update_status(&id_killed, TaskStatus::Killed).await;

    let out = mgr.collect(None, true).await;
    assert_eq!(
        out.len(),
        4,
        "running + 3 terminal tasks must all appear in post-compact reminder"
    );

    let by_id: std::collections::HashMap<&str, &coco_system_reminder::TaskStatusSnapshot> =
        out.iter().map(|s| (s.task_id.as_str(), s)).collect();

    use coco_system_reminder::TaskRunStatus;
    assert!(matches!(
        by_id[id_running.as_str()].status,
        TaskRunStatus::Running
    ));
    assert!(matches!(
        by_id[id_completed.as_str()].status,
        TaskRunStatus::Completed
    ));
    assert!(matches!(
        by_id[id_failed.as_str()].status,
        TaskRunStatus::Failed
    ));
    assert!(matches!(
        by_id[id_killed.as_str()].status,
        TaskRunStatus::Killed
    ));
}

/// Pending tasks are filtered out — the model spawned them but they
/// haven't started running yet, so there's nothing to report.
#[tokio::test]
async fn collect_skips_pending_tasks_post_compact() {
    let mgr = TaskManager::new();
    let _id = create_pending(&mgr, TaskType::BgAgent, "queued", "/tmp/queued.log").await;
    let out = mgr.collect(None, true).await;
    assert!(
        out.is_empty(),
        "Pending tasks must not appear in post-compact reminder"
    );
}

#[tokio::test]
async fn status_mapping_collapses_5_to_4() {
    // 5 statuses exist; the reminder generator
    // dispatches on 4 (Pending/Running both render as Running).
    assert!(matches!(
        map_status(coco_types::TaskStatus::Completed),
        coco_system_reminder::TaskRunStatus::Completed
    ));
    assert!(matches!(
        map_status(coco_types::TaskStatus::Failed),
        coco_system_reminder::TaskRunStatus::Failed
    ));
    assert!(matches!(
        map_status(coco_types::TaskStatus::Killed),
        coco_system_reminder::TaskRunStatus::Killed
    ));
    assert!(matches!(
        map_status(coco_types::TaskStatus::Running),
        coco_system_reminder::TaskRunStatus::Running
    ));
    assert!(matches!(
        map_status(coco_types::TaskStatus::Pending),
        coco_system_reminder::TaskRunStatus::Running
    ));
}
