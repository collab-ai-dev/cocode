use tracing::info;

use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};
use crate::session_labels::SessionLabelError;

/// `session/rename` — persist a user-visible title for the active session.
pub(crate) async fn handle_session_rename(
    params: coco_types::SessionRenameParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let Some(runtime) = runtime else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session runtime".into(),
            data: None,
        };
    };
    match crate::session_labels::rename_session(&runtime, params.name).await {
        Ok(result) => {
            info!(name = %result.name, "AppServerHost: session/rename");
            HandlerResult::ok(result)
        }
        Err(error) => session_label_error(error),
    }
}

/// `session/toggleTag` — toggle a tag on the active persisted session.
pub(crate) async fn handle_session_toggle_tag(
    params: coco_types::SessionToggleTagParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let runtime = ctx.resolve_runtime().await;
    let Some(runtime) = runtime else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session runtime".into(),
            data: None,
        };
    };
    let session_id = runtime.session_id().clone();
    match crate::session_labels::toggle_session_tag(&runtime, params.tag).await {
        Ok(result) => {
            info!(
                session_id = %session_id,
                tag = %result.tag,
                added = result.added,
                "AppServerHost: session/toggleTag"
            );
            HandlerResult::ok(result)
        }
        Err(error) => session_label_error(error),
    }
}

fn session_label_error(error: SessionLabelError) -> HandlerResult {
    match error {
        SessionLabelError::Rename(
            error @ (crate::session_rename::RenamePersistenceError::EmptyName
            | crate::session_rename::RenamePersistenceError::TranscriptNotFound),
        ) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: error.user_message(),
            data: None,
        },
        SessionLabelError::Rename(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: error.user_message(),
            data: None,
        },
        error @ (SessionLabelError::AutoRenameRuntimeRequired
        | SessionLabelError::AutoRename(_)) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: error.user_message(),
            data: None,
        },
        error @ SessionLabelError::EmptyTag => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: error.to_string(),
            data: None,
        },
        error @ SessionLabelError::ToggleTag { .. } => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: error.to_string(),
            data: None,
        },
    }
}
