use tracing::info;

use super::session_control_error;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_controls;

/// `control/stopTask` — cooperative cancellation of a specific task.
///
/// When the local AppServer bridge has installed a [`SessionRuntime`],
/// route through its task registry so the target task's cancel token fires.
pub(crate) async fn handle_stop_task(
    params: coco_types::StopTaskParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let session_id = runtime.as_ref().map(|runtime| runtime.session_id().clone());
    match session_controls::stop_task(runtime, &params.task_id).await {
        Ok(()) => {
            info!(
                session_id = ?session_id,
                task_id = %params.task_id,
                "AppServerHost: control/stopTask"
            );
            HandlerResult::ok_empty()
        }
        Err(error) => session_control_error(error),
    }
}

/// `agent/interruptCurrentWork` — abort one teammate's current turn
/// without killing the teammate lifecycle.
///
/// Escape while viewing a teammate aborts the current work controller,
/// whereas Ctrl+C still kills agents via the broader cancellation path.
pub(crate) async fn handle_agent_interrupt_current_work(
    params: coco_types::AgentInterruptCurrentWorkParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_controls::interrupt_agent_current_work(
        ctx.resolve_runtime().await,
        &params.agent_id,
    )
    .await
    {
        Ok(()) => HandlerResult::ok_empty(),
        Err(error) => session_control_error(error),
    }
}

/// `task/list` — list running/background tasks for the active session.
pub(crate) async fn handle_task_list(ctx: &HandlerContext) -> HandlerResult {
    match session_controls::list_tasks(ctx.resolve_runtime().await).await {
        Ok(result) => {
            info!(count = result.tasks.len(), "AppServerHost: task/list");
            HandlerResult::ok(result)
        }
        Err(error) => session_control_error(error),
    }
}

/// `task/detail` — read terminal outputs for one running/background task.
pub(crate) async fn handle_task_detail(
    params: coco_types::TaskDetailParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    match session_controls::task_detail(ctx.resolve_runtime().await, params.task_id).await {
        Ok(result) => {
            let task_id = result.task_id.clone();
            info!(task_id = %task_id, "AppServerHost: task/detail");
            HandlerResult::ok(result)
        }
        Err(error) => session_control_error(error),
    }
}

/// `control/backgroundAllTasks` — detach every foreground task into the
/// background. No-op when this session has no task runtime installed.
pub(crate) async fn handle_background_all_tasks(ctx: &HandlerContext) -> HandlerResult {
    let result = session_controls::background_all_tasks(ctx.resolve_runtime().await).await;
    info!(
        count = result.task_ids.len(),
        "AppServerHost: control/backgroundAllTasks"
    );
    HandlerResult::ok(result)
}
