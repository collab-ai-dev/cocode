use std::{sync::Arc, time::Duration};

use tracing::warn;

use crate::session_runtime::SessionHandle;

pub(crate) struct ArchivedSession {
    pub(crate) result: coco_types::SessionResultParams,
}

pub(crate) fn build_session_result(
    session: &SessionHandle,
    default_stop_reason: &str,
) -> coco_types::SessionResultParams {
    let crate::session_runtime::SessionAccounting { started_at, stats } =
        session.session_accounting_snapshot();
    coco_types::SessionResultParams {
        session_id: session.session_id().clone(),
        total_turns: stats.total_turns,
        duration_ms: started_at.elapsed().as_millis() as i64,
        duration_api_ms: stats.total_duration_api_ms,
        is_error: stats.had_error,
        stop_reason: stats
            .last_stop_reason
            .clone()
            .unwrap_or_else(|| default_stop_reason.into()),
        total_cost_usd: stats.total_cost_usd,
        usage: stats.usage,
        model_usage: stats.model_usage.clone(),
        permission_denials: stats.permission_denials.clone(),
        result: stats.last_result_text.clone(),
        errors: stats.errors.clone(),
        structured_output: if stats.had_error {
            None
        } else {
            stats.structured_output.clone()
        },
        fast_mode_state: None,
        num_api_calls: if stats.num_api_calls > 0 {
            Some(stats.num_api_calls)
        } else {
            None
        },
    }
}

pub(crate) async fn archive_live_session(
    session: &SessionHandle,
    session_manager: Option<Arc<coco_session::SessionManager>>,
    turn_drain_timeout: Duration,
) -> ArchivedSession {
    let result = build_session_result(session, "archived");
    let session_id = session.session_id().clone();
    let active_turn = session.take_active_turn();

    if let Some(active_turn) = &active_turn {
        active_turn.cancel_token.cancel();
    }

    if let Some(active_turn) = active_turn {
        drain_archive_turn(&session_id, active_turn, turn_drain_timeout).await;
    }

    delete_persisted_session_record(&session_id, session_manager).await;

    ArchivedSession { result }
}

async fn drain_archive_turn(
    session_id: &coco_types::SessionId,
    active_turn: crate::session_runtime::ActiveTurnHandles,
    timeout: Duration,
) {
    match tokio::time::timeout(timeout, active_turn.turn_task).await {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) => warn!(
            session_id = %session_id,
            error = %join_err,
            "session/archive: turn task join failed"
        ),
        Err(_) => warn!(
            session_id = %session_id,
            "session/archive: turn task did not exit within timeout of cancel; \
             emitting aggregate anyway (late events may still follow)"
        ),
    }
    match tokio::time::timeout(timeout, active_turn.forwarder_task).await {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) => warn!(
            session_id = %session_id,
            error = %join_err,
            "session/archive: forwarder task join failed"
        ),
        Err(_) => warn!(
            session_id = %session_id,
            "session/archive: forwarder task did not drain within timeout"
        ),
    }
}

async fn delete_persisted_session_record(
    session_id: &coco_types::SessionId,
    session_manager: Option<Arc<coco_session::SessionManager>>,
) {
    let Some(manager) = session_manager else {
        return;
    };
    let target_id = session_id.as_str().to_string();
    let delete_result = tokio::task::spawn_blocking(move || manager.delete(&target_id)).await;
    match delete_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(
            session_id = %session_id,
            error = %error,
            "session/archive: failed to delete persisted session record"
        ),
        Err(join_err) => warn!(
            session_id = %session_id,
            error = %join_err,
            "session/archive: delete task panicked"
        ),
    }
}
