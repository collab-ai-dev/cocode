use tracing::info;

use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};

pub(crate) async fn handle_session_delete(
    params: coco_types::SessionDeleteParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let session_id = params.target.session_id;
    // `begin_delete` fails while any slot exists AND blocks every slot
    // reservation until `finish_delete`, so a concurrent resume cannot
    // publish a live runtime over rows mid-deletion (and vice versa).
    if let Some(app_server) = &ctx.app_server
        && let Err(error) = app_server.registry().begin_delete(&session_id)
    {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("session {session_id} is still live: {error}"),
            data: Some(serde_json::json!({
                "kind": "SessionStillLive",
                "session_id": session_id,
            })),
        };
    }

    let result = delete_durable_session(&session_id, ctx).await;
    if let Some(app_server) = &ctx.app_server {
        app_server.registry().finish_delete(&session_id);
    }
    result
}

async fn delete_durable_session(
    session_id: &coco_types::SessionId,
    ctx: &HandlerContext,
) -> HandlerResult {
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
                app_server.revoke_session_grants(session_id);
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
