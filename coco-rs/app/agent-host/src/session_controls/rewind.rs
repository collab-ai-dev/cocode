use crate::session_runtime::{
    SessionFileDiffError, SessionFileRewindError, SessionFileRewindRequest,
    SessionFileRewindResult, SessionHandle,
};

use super::{SessionControlError, require_runtime};

pub enum FileHistoryDiffTarget<'a> {
    Session,
    Turn { message_id: &'a str },
}

pub async fn rewind_files(
    runtime: Option<SessionHandle>,
    user_message_id: String,
    dry_run: bool,
) -> Result<SessionFileRewindResult, SessionControlError> {
    let runtime = require_runtime(runtime, "control/rewindFiles")?;
    let request = SessionFileRewindRequest {
        user_message_id,
        dry_run,
    };
    runtime
        .rewind_files(request)
        .await
        .map_err(|error| match error {
            SessionFileRewindError::NotEnabled => SessionControlError::FileHistoryNotEnabled,
            SessionFileRewindError::SnapshotMissing(user_message_id) => {
                SessionControlError::FileRewindSnapshotMissing(user_message_id)
            }
            SessionFileRewindError::Operation { context, source } => {
                SessionControlError::FileRewindOperation { context, source }
            }
        })
}

pub async fn file_history_diff(
    runtime: Option<SessionHandle>,
    target: FileHistoryDiffTarget<'_>,
) -> Result<coco_context::RenderedDiff, SessionControlError> {
    let runtime = require_runtime(runtime, "file history diff")?;
    let result = match target {
        FileHistoryDiffTarget::Session => runtime.render_session_file_diff().await,
        FileHistoryDiffTarget::Turn { message_id } => {
            runtime.render_turn_file_diff(message_id).await
        }
    };
    result.map_err(file_history_diff_error)
}

pub async fn rewind_diff_stats(
    runtime: Option<SessionHandle>,
    message_id: &str,
) -> Result<Option<coco_context::DiffStats>, SessionControlError> {
    rewind_diff_stats_between(runtime, message_id, None).await
}

pub async fn rewind_diff_stats_between(
    runtime: Option<SessionHandle>,
    message_id: &str,
    next_message_id: Option<&str>,
) -> Result<Option<coco_context::DiffStats>, SessionControlError> {
    let runtime = require_runtime(runtime, "rewind diff stats")?;
    runtime
        .rewind_diff_stats_between(message_id, next_message_id)
        .await
        .map_err(file_history_diff_error)
}

fn file_history_diff_error(error: SessionFileDiffError) -> SessionControlError {
    match error {
        SessionFileDiffError::NotEnabled => SessionControlError::FileDiffNotEnabled,
        SessionFileDiffError::SnapshotMissing(message_id) => {
            SessionControlError::FileDiffSnapshotMissing(message_id)
        }
        SessionFileDiffError::Operation { context, source } => {
            SessionControlError::FileDiffOperation { context, source }
        }
    }
}
