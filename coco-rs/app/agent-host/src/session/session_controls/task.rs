use crate::session_runtime::{SessionHandle, SessionTaskError};

use super::{SessionControlError, require_runtime};

pub struct TurnInterruptResult {
    pub session_id: coco_types::SessionId,
}

pub async fn stop_task(
    runtime: Option<SessionHandle>,
    task_id: &str,
) -> Result<(), SessionControlError> {
    let runtime = require_runtime(runtime, "control/stopTask")?;
    runtime
        .stop_session_task(task_id)
        .await
        .map_err(|error| match error {
            SessionTaskError::NotAvailable => SessionControlError::TaskRuntimeUnavailable,
            error => SessionControlError::Task {
                operation: "control/stopTask",
                source: error,
            },
        })
}

pub async fn interrupt_agent_current_work(
    runtime: Option<SessionHandle>,
    agent_id: &str,
) -> Result<(), SessionControlError> {
    let runtime = require_runtime(runtime, "agent/interruptCurrentWork")?;
    match runtime.interrupt_agent_current_work(agent_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(SessionControlError::AgentInterrupt(format!(
            "agent {agent_id} has no active current work to interrupt"
        ))),
        Err(message) => Err(SessionControlError::AgentInterrupt(message)),
    }
}

pub async fn interrupt_active_turn(
    runtime: Option<SessionHandle>,
) -> Result<TurnInterruptResult, SessionControlError> {
    let runtime = require_runtime(runtime, "turn/interrupt")?;
    let session_id = runtime.session_id().clone();
    let Some(token) = runtime.active_turn_cancel_token() else {
        return Err(SessionControlError::NoActiveTurn);
    };
    token.cancel();
    Ok(TurnInterruptResult { session_id })
}

pub async fn list_tasks(
    runtime: Option<SessionHandle>,
) -> Result<coco_types::TaskListResult, SessionControlError> {
    let runtime = require_runtime(runtime, "task/list")?;
    let Some(tasks) = runtime.list_session_tasks().await else {
        return Err(SessionControlError::TaskRuntimeUnavailable);
    };
    Ok(coco_types::TaskListResult { tasks })
}

pub async fn task_detail(
    runtime: Option<SessionHandle>,
    task_id: String,
) -> Result<coco_types::TaskDetailResult, SessionControlError> {
    let runtime = require_runtime(runtime, "task/detail")?;
    match runtime.read_session_task_outputs(&task_id).await {
        Ok(outputs) => Ok(coco_types::TaskDetailResult {
            task_id,
            stdout: outputs.stdout,
            stderr: outputs.stderr,
            exit_code: outputs.exit_code,
            interrupted: outputs.interrupted,
        }),
        Err(SessionTaskError::NotAvailable) => Err(SessionControlError::TaskRuntimeUnavailable),
        Err(error) => Err(SessionControlError::Task {
            operation: "task/detail",
            source: error,
        }),
    }
}

pub async fn background_all_tasks(
    runtime: Option<SessionHandle>,
) -> coco_types::BackgroundAllTasksResult {
    let Some(runtime) = runtime else {
        return coco_types::BackgroundAllTasksResult {
            task_ids: Vec::new(),
        };
    };
    let task_ids = runtime.background_all_session_tasks().await;
    coco_types::BackgroundAllTasksResult { task_ids }
}
