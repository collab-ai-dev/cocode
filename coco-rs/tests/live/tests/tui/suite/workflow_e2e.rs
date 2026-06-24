//! Workflow launch through the TUI harness.
//!
//! The parent scripted model calls the real `Workflow` tool, the workflow
//! runtime emits background task progress, and the TUI folds those task events
//! into `AppState.active_tasks`.

use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TaskCompletionStatus;
use coco_types::task_type_wire;
use serde_json::json;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

const WORKFLOW_NAME: &str = "release-e2e";

const WORKFLOW_SCRIPT: &str = r#"
export const meta = {
  name: "release-e2e",
  description: "TUI workflow e2e",
  phases: [{ name: "Plan", description: "Plan release" }]
};

phase("Plan");
log(`target=${args.target}`);
return {
  ok: true,
  target: args.target,
  spent: budget.spent()
};
"#;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_workflow_tool()
        .with_replies([
            Reply::tool_call(
                "call-workflow",
                "Workflow",
                json!({
                    "script": WORKFLOW_SCRIPT,
                    "args": { "target": "release" },
                }),
            ),
            Reply::text("Launched the release workflow."),
        ])
        .with_max_turns(6)
        .build()
        .await?;

    harness.submit("run the release workflow").await;
    let approval = harness
        .pump_until_approval_request(Duration::from_secs(10))
        .await?;
    assert_eq!(
        approval.tool_name, "Workflow",
        "workflow_e2e: inline workflow should request Workflow approval"
    );
    assert!(
        approval.input_preview.contains(WORKFLOW_NAME),
        "workflow_e2e: approval preview should include workflow source/name: {}",
        approval.input_preview
    );
    assert!(
        harness.approve(&approval.request_id).await,
        "workflow_e2e: approval request should resolve"
    );
    let clean = harness.pump_until_idle(Duration::from_secs(10)).await?;
    assert!(clean, "workflow_e2e: SessionResult flagged is_error");

    wait_for_workflow_completion(&mut harness).await?;

    assert_eq!(
        harness.tool_starts(),
        vec!["Workflow"],
        "workflow_e2e: expected one Workflow tool start"
    );
    assert_eq!(
        harness.tool_completions(),
        vec![("Workflow", false)],
        "workflow_e2e: Workflow should complete cleanly"
    );

    let (tool_result, is_error) = harness
        .find_tool_result("Workflow")
        .ok_or_else(|| anyhow!("workflow_e2e: missing Workflow tool result"))?;
    assert!(
        !is_error,
        "workflow_e2e: Workflow tool result should be success: {tool_result}"
    );
    assert!(
        tool_result.contains("taskType: local_workflow")
            && tool_result.contains(&format!("workflow: {WORKFLOW_NAME}")),
        "workflow_e2e: Workflow tool result missing launch metadata: {tool_result}"
    );

    let task_id = workflow_task_id(&harness)?;
    assert!(
        harness.events.iter().any(|event| matches!(
            event,
            CoreEvent::Protocol(ServerNotification::TaskCompleted(params))
                if params.task_id == task_id
                    && matches!(params.status, TaskCompletionStatus::Completed)
        )),
        "workflow_e2e: missing completed workflow task event"
    );

    let task = harness
        .state
        .session
        .active_tasks
        .iter()
        .find(|task| task.task_id == task_id)
        .ok_or_else(|| anyhow!("workflow_e2e: TUI active_tasks missing workflow row"))?;
    assert_eq!(
        task.kind,
        coco_tui::state::session::TaskEntryKind::Workflow,
        "workflow_e2e: TUI should classify local_workflow as Workflow"
    );
    assert_eq!(
        task.workflow_name.as_deref(),
        Some(WORKFLOW_NAME),
        "workflow_e2e: TUI should preserve workflow name"
    );
    assert!(
        task.workflow_progress.iter().any(|event| matches!(
            event,
            coco_types::WorkflowProgressEvent::WorkflowPhase { title, .. } if title == "Plan"
        )),
        "workflow_e2e: TUI workflow row missing phase progress: {:?}",
        task.workflow_progress
    );
    assert!(
        task.workflow_progress.iter().any(|event| matches!(
            event,
            coco_types::WorkflowProgressEvent::WorkflowLog { message } if message == "target=release"
        )),
        "workflow_e2e: TUI workflow row missing log progress: {:?}",
        task.workflow_progress
    );

    harness.shutdown().await;
    Ok(())
}

async fn wait_for_workflow_completion(harness: &mut TuiHarness) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        harness
            .drain_pending_events(Duration::from_millis(50))
            .await;
        if harness.events.iter().any(|event| {
            matches!(
                event,
                CoreEvent::Protocol(ServerNotification::TaskCompleted(params))
                    if matches!(params.status, TaskCompletionStatus::Completed)
            )
        }) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "workflow_e2e: timed out waiting for workflow completion; last event={:?}",
                harness.events.last()
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn workflow_task_id(harness: &TuiHarness) -> Result<String> {
    harness
        .events
        .iter()
        .find_map(|event| match event {
            CoreEvent::Protocol(ServerNotification::TaskStarted(params))
                if params.task_type.as_deref() == Some(task_type_wire::LOCAL_WORKFLOW)
                    && params.workflow_name.as_deref() == Some(WORKFLOW_NAME) =>
            {
                Some(params.task_id.clone())
            }
            _ => None,
        })
        .ok_or_else(|| anyhow!("workflow_e2e: missing local_workflow TaskStarted event"))
}
