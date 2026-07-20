use tracing::info;

use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};

pub(crate) async fn handle_session_delete(
    params: coco_types::SessionDeleteParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let session_id = params.target.session_id;
    if ctx
        .app_server
        .as_ref()
        .is_some_and(|app_server| app_server.has_session_slot(&session_id))
    {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("session {session_id} is still live"),
            data: Some(serde_json::json!({
                "kind": "SessionStillLive",
                "session_id": session_id,
            })),
        };
    }

    let Some(manager) = ctx.state.session_manager_snapshot().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/delete requires persisted session storage".to_string(),
            data: Some(serde_json::json!({ "kind": "session_storage_unavailable" })),
        };
    };

    let target_id = session_id.as_str().to_string();
    match tokio::task::spawn_blocking(move || manager.delete(&target_id)).await {
        Ok(Ok(())) => {
            if let Some(app_server) = &ctx.app_server {
                app_server.revoke_session_grants(&session_id);
            }
            info!(session_id = %session_id, "AppServerHost: session/delete");
            HandlerResult::ok_empty()
        }
        Ok(Err(error)) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("failed to delete session {session_id}: {error}"),
            data: Some(serde_json::json!({ "kind": "session_delete_failed" })),
        },
        Err(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("session/delete task failed for {session_id}: {error}"),
            data: Some(serde_json::json!({ "kind": "session_delete_task_failed" })),
        },
    }
}
