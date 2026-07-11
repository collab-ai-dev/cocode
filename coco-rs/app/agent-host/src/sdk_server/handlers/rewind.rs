//! `control/rewindFiles` — restore tracked files to a named snapshot.
//!
//! File history and `config_home` are resolved from the request's targeted
//! session runtime.

use tracing::info;

use super::{HandlerContext, HandlerResult};

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
pub(super) async fn handle_rewind_files(
    params: coco_types::RewindFilesParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    // Resolve the active session_id.
    let Some(session_id) = ctx.active_session_id().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session; call session/start first".into(),
            data: None,
        };
    };

    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "control/rewindFiles requires a live targeted session".into(),
            data: None,
        };
    };
    let Some(history_arc) = runtime.file_history().cloned() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "control/rewindFiles: file history not enabled on this server".into(),
            data: None,
        };
    };
    let config_home = runtime.config_home().clone();

    // Verify the snapshot exists before attempting the operation —
    // gives a clearer error than "rewind failed: not found".
    {
        let history = history_arc.read().await;
        if !history.can_restore(&params.user_message_id) {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_REQUEST,
                message: format!(
                    "control/rewindFiles: no snapshot for user_message_id {}",
                    params.user_message_id
                ),
                data: None,
            };
        }
    }

    if params.dry_run {
        // Preview path — get diff stats without touching disk.
        let history = history_arc.read().await;
        let stats = match history
            .get_diff_stats(&params.user_message_id, &config_home, session_id.as_str())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("control/rewindFiles dry_run: {e}"),
                    data: None,
                };
            }
        };
        info!(
            user_message_id = %params.user_message_id,
            files = stats.files_changed.len(),
            "SdkServer: control/rewindFiles (dry_run)"
        );
        HandlerResult::ok(coco_types::RewindFilesResult {
            files_changed: stats
                .files_changed
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            insertions: stats.insertions,
            deletions: stats.deletions,
            dry_run: true,
        })
    } else {
        // Apply path — get diff stats first for the response payload,
        // then perform the rewind.
        let stats = {
            let history = history_arc.read().await;
            match history
                .get_diff_stats(&params.user_message_id, &config_home, session_id.as_str())
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    return HandlerResult::Err {
                        code: coco_types::error_codes::INTERNAL_ERROR,
                        message: format!("control/rewindFiles preview: {e}"),
                        data: None,
                    };
                }
            }
        };
        let history = history_arc.read().await;
        let restored = match history
            .rewind(&params.user_message_id, &config_home, session_id.as_str())
            .await
        {
            Ok(paths) => paths,
            Err(e) => {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("control/rewindFiles: {e}"),
                    data: None,
                };
            }
        };
        info!(
            user_message_id = %params.user_message_id,
            files = restored.len(),
            "SdkServer: control/rewindFiles (applied)"
        );
        HandlerResult::ok(coco_types::RewindFilesResult {
            files_changed: restored
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            insertions: stats.insertions,
            deletions: stats.deletions,
            dry_run: false,
        })
    }
}
