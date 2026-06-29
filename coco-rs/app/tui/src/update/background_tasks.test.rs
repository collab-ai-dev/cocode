use tokio::sync::mpsc;

use super::intercept;
use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::BackgroundTasksState;
use crate::state::ModalState;
use crate::state::WorkflowAgentStatusFilter;
use crate::state::session::TaskEntry;
use crate::state::session::TaskEntryKind;
use crate::state::session::TaskEntryStatus;

fn running_task(task_id: &str, kind: TaskEntryKind) -> TaskEntry {
    TaskEntry {
        task_id: task_id.into(),
        description: task_id.into(),
        status: TaskEntryStatus::Running,
        kind,
        started_at_ms: 0,
        workflow_name: if kind == TaskEntryKind::Workflow {
            Some(task_id.into())
        } else {
            None
        },
        workflow_progress: Vec::new(),
    }
}

#[tokio::test]
async fn x_cancels_selected_workflow_task() {
    let mut state = AppState::default();
    state.session.active_tasks = vec![
        running_task("agent-1", TaskEntryKind::Agent),
        running_task("shell-1", TaskEntryKind::Shell),
        running_task("workflow-1", TaskEntryKind::Workflow),
    ];
    state
        .ui
        .show_modal(ModalState::BackgroundTasks(BackgroundTasksState {
            selected: 2,
            detail: None,
            workflow_agent_filter: Default::default(),
        }));
    let (tx, mut rx) = mpsc::channel(4);

    let handled = intercept(&mut state, &TuiCommand::InsertChar('x'), &tx).await;

    assert!(matches!(handled, super::Handled::Yes(true)));
    let cmd = rx.try_recv().expect("cancel command sent");
    assert!(matches!(
        cmd,
        UserCommand::CancelSubagent { task_id } if task_id == "workflow-1"
    ));
}

#[tokio::test]
async fn f_cycles_workflow_detail_filter_skipping_empty_statuses() {
    let mut state = AppState::default();
    let mut workflow = running_task("workflow-1", TaskEntryKind::Workflow);
    workflow.workflow_progress = vec![
        coco_types::WorkflowProgressEvent::WorkflowAgent {
            index: 0,
            state: coco_types::WorkflowAgentState::Done,
            label: "Explore".to_string(),
            phase_title: None,
            phase_index: None,
            agent_id: None,
            model: None,
            started_at: None,
            queued_at: None,
            last_progress_at: None,
            tokens: None,
            tool_calls: None,
            duration_ms: None,
            cached: false,
            result_preview: None,
            prompt_preview: None,
            error: None,
            skipped: false,
        },
        coco_types::WorkflowProgressEvent::WorkflowAgent {
            index: 1,
            state: coco_types::WorkflowAgentState::Error,
            label: "Verify".to_string(),
            phase_title: None,
            phase_index: None,
            agent_id: None,
            model: None,
            started_at: None,
            queued_at: None,
            last_progress_at: None,
            tokens: None,
            tool_calls: None,
            duration_ms: None,
            cached: false,
            result_preview: None,
            prompt_preview: None,
            error: Some("failed".to_string()),
            skipped: false,
        },
    ];
    state.session.active_tasks = vec![workflow];
    state
        .ui
        .show_modal(ModalState::BackgroundTasks(BackgroundTasksState {
            selected: 0,
            detail: Some("workflow-1".to_string()),
            workflow_agent_filter: WorkflowAgentStatusFilter::All,
        }));
    let (tx, _rx) = mpsc::channel(4);

    let handled = intercept(&mut state, &TuiCommand::InsertChar('f'), &tx).await;

    assert!(matches!(handled, super::Handled::Yes(true)));
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(bt))
            if bt.workflow_agent_filter == WorkflowAgentStatusFilter::Failed
    ));

    let handled = intercept(&mut state, &TuiCommand::InsertChar('f'), &tx).await;

    assert!(matches!(handled, super::Handled::Yes(true)));
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(bt))
            if bt.workflow_agent_filter == WorkflowAgentStatusFilter::Done
    ));
}

#[tokio::test]
async fn f_cycles_to_queued_and_skipped_workflow_statuses() {
    let mut state = AppState::default();
    let mut workflow = running_task("workflow-1", TaskEntryKind::Workflow);
    workflow.workflow_progress = vec![
        coco_types::WorkflowProgressEvent::WorkflowAgent {
            index: 0,
            state: coco_types::WorkflowAgentState::Start,
            label: "Explore".to_string(),
            phase_title: None,
            phase_index: None,
            agent_id: None,
            model: None,
            started_at: None,
            queued_at: Some(1_700_000_000_000),
            last_progress_at: None,
            tokens: None,
            tool_calls: None,
            duration_ms: None,
            cached: false,
            result_preview: None,
            prompt_preview: None,
            error: None,
            skipped: false,
        },
        coco_types::WorkflowProgressEvent::WorkflowAgent {
            index: 1,
            state: coco_types::WorkflowAgentState::Error,
            label: "Verify".to_string(),
            phase_title: None,
            phase_index: None,
            agent_id: None,
            model: None,
            started_at: None,
            queued_at: None,
            last_progress_at: None,
            tokens: None,
            tool_calls: None,
            duration_ms: None,
            cached: false,
            result_preview: None,
            prompt_preview: None,
            error: Some("skipped by user".to_string()),
            skipped: true,
        },
    ];
    state.session.active_tasks = vec![workflow];
    state
        .ui
        .show_modal(ModalState::BackgroundTasks(BackgroundTasksState {
            selected: 0,
            detail: Some("workflow-1".to_string()),
            workflow_agent_filter: WorkflowAgentStatusFilter::All,
        }));
    let (tx, _rx) = mpsc::channel(4);

    let handled = intercept(&mut state, &TuiCommand::InsertChar('f'), &tx).await;

    assert!(matches!(handled, super::Handled::Yes(true)));
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(bt))
            if bt.workflow_agent_filter == WorkflowAgentStatusFilter::Queued
    ));

    let handled = intercept(&mut state, &TuiCommand::InsertChar('f'), &tx).await;

    assert!(matches!(handled, super::Handled::Yes(true)));
    assert!(matches!(
        state.ui.modal.as_ref(),
        Some(ModalState::BackgroundTasks(bt))
            if bt.workflow_agent_filter == WorkflowAgentStatusFilter::Skipped
    ));
}
