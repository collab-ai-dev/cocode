use tokio::sync::mpsc;

use super::intercept;
use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::BackgroundTasksState;
use crate::state::ModalState;
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
