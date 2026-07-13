//! `control/rewindFiles` — restore tracked files to a named snapshot.
//!
//! File history and `config_home` are resolved from the request's targeted
//! session runtime.

use tracing::info;

use super::{HandlerContext, HandlerResult};
use crate::session_controls::{self, SessionControlError};

/// `control/rewindFiles` — restore tracked files to a snapshot keyed
/// by `user_message_id`.
///
/// In `dry_run=true` mode, returns a preview (file list + diff stats)
/// without modifying disk. In `dry_run=false` mode, performs the
/// actual restore by writing the backed-up file contents back to
/// their original paths.
///
/// Requires:
/// - An active session (for the session_id used to key file backups)
/// - File history enabled on the targeted session runtime
///
/// Errors:
/// - `INVALID_REQUEST` if no active session
/// - `INVALID_REQUEST` if file history is not enabled on this server
/// - `INVALID_REQUEST` if `user_message_id` doesn't match any snapshot
/// - `INTERNAL_ERROR` if the rewind / diff operation fails (filesystem)
pub(crate) async fn handle_rewind_files(
    params: coco_types::RewindFilesParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let user_message_id = params.user_message_id.clone();
    let result = match session_controls::rewind_files(
        ctx.resolve_runtime().await,
        params.user_message_id,
        params.dry_run,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return rewind_files_error(error),
    };
    info!(
        user_message_id = %user_message_id,
        files = result.files_changed.len(),
        dry_run = result.dry_run,
        "AppServerHost: control/rewindFiles"
    );
    HandlerResult::ok(coco_types::RewindFilesResult {
        files_changed: result
            .files_changed
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        insertions: result.insertions,
        deletions: result.deletions,
        dry_run: result.dry_run,
    })
}

fn rewind_files_error(error: SessionControlError) -> HandlerResult {
    match error {
        SessionControlError::ActiveRuntimeRequired { .. } => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "control/rewindFiles requires a live targeted session".into(),
            data: None,
        },
        SessionControlError::FileHistoryNotEnabled => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "control/rewindFiles: file history not enabled on this server".into(),
            data: None,
        },
        SessionControlError::FileRewindSnapshotMissing(user_message_id) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "control/rewindFiles: no snapshot for user_message_id {user_message_id}"
            ),
            data: None,
        },
        SessionControlError::FileRewindOperation { context, source } => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("control/rewindFiles {context}: {source}"),
            data: None,
        },
        error => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: error.to_string(),
            data: None,
        },
    }
}
