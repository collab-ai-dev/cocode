use super::*;
use coco_tasks::TaskCreateRequest;
use coco_types::{TaskKilledBy, TaskStatus, TaskType};

async fn running_agent() -> (std::sync::Arc<TaskManager>, String, CancellationToken) {
    let manager = std::sync::Arc::new(TaskManager::new());
    let task_id = "a_watchdog_test".to_string();
    let cancel = CancellationToken::new();
    manager
        .create_task(TaskCreateRequest {
            task_id: task_id.clone(),
            task_type: TaskType::BgAgent,
            description: "watchdog test".to_string(),
            output_file: None,
            tool_use_id: None,
            is_backgrounded: true,
            status: TaskStatus::Running,
            cancel: cancel.clone(),
            invoking_agent: None,
            workflow_run_id: String::new(),
            workflow_name: None,
            workflow_prompt: None,
            shell_extras: None,
        })
        .await;
    (manager, task_id, cancel)
}

fn fast_policy() -> AgentLivenessPolicy {
    AgentLivenessPolicy {
        model_warning_after: Duration::from_secs(2),
        model_timeout_after: Duration::from_secs(4),
        tool_warning_after: Duration::from_secs(5),
        tool_timeout_after: Duration::from_secs(8),
        absolute_timeout: Duration::from_secs(20),
    }
}

#[tokio::test(start_paused = true)]
async fn model_inactivity_cancels_agent() {
    let (manager, task_id, cancel) = running_agent().await;
    tokio::spawn(watch_agent_liveness(
        task_id.clone(),
        manager.clone(),
        cancel.clone(),
        fast_policy(),
    ));
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;

    assert!(cancel.is_cancelled());
    let state = manager.get(&task_id).await.expect("task row");
    assert_eq!(state.killed_by, Some(TaskKilledBy::System));
}

#[tokio::test(start_paused = true)]
async fn tool_activity_uses_longer_timeout_and_refreshes_deadline() {
    let (manager, task_id, cancel) = running_agent().await;
    tokio::spawn(watch_agent_liveness(
        task_id.clone(),
        manager.clone(),
        cancel.clone(),
        fast_policy(),
    ));
    tokio::task::yield_now().await;
    manager
        .record_agent_activity(&task_id, AgentExecutionPhase::RunningTool)
        .await;
    tokio::time::advance(Duration::from_secs(4)).await;
    manager
        .record_agent_activity(&task_id, AgentExecutionPhase::RunningTool)
        .await;
    tokio::task::yield_now().await;
    assert!(!cancel.is_cancelled());

    tokio::time::advance(Duration::from_secs(9)).await;
    tokio::task::yield_now().await;
    assert!(cancel.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn continuous_activity_cannot_bypass_absolute_timeout() {
    let (manager, task_id, cancel) = running_agent().await;
    tokio::spawn(watch_agent_liveness(
        task_id.clone(),
        manager.clone(),
        cancel.clone(),
        fast_policy(),
    ));
    tokio::task::yield_now().await;

    manager
        .record_agent_activity(&task_id, AgentExecutionPhase::RunningTool)
        .await;
    for _ in 0..6 {
        tokio::time::advance(Duration::from_secs(3)).await;
        manager
            .record_agent_activity(&task_id, AgentExecutionPhase::RunningTool)
            .await;
        tokio::task::yield_now().await;
    }
    assert!(!cancel.is_cancelled());

    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;

    assert!(cancel.is_cancelled());
    let state = manager.get(&task_id).await.expect("task row");
    assert_eq!(state.killed_by, Some(TaskKilledBy::System));
}
