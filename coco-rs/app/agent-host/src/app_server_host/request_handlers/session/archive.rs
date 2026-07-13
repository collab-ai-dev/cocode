use coco_types::CoreEvent;
use tracing::info;

use crate::app_server_host::outbound::send_session_event_and_wait;
use crate::app_server_host::request_handlers::{HandlerContext, HandlerResult};

/// `session/archive` — clear keyed session state.
///
/// Emits the aggregated `SessionResult` (built from the session's
/// accumulated stats) as a final notification before clearing session state.
/// This gives remote clients exactly one `SessionResult` per session,
/// regardless of how many `turn/start` calls happened inside it.
///
/// **Ordering note**: The `SessionResult` notification and archive
/// response both go through the dispatcher's ordered outbound queue, so
/// the client sees the aggregate before the JSON-RPC response.
///
/// **Archive-during-running-turn**: If a turn is in flight when
/// `session/archive` is called, the aggregate is built from whatever
/// stats have been accumulated so far (the in-flight turn's stats are
/// NOT included — it's cancelled after the aggregate is built). This
/// Archive discards in-progress work.
///
/// Errors:
/// - `INVALID_REQUEST` if no session is active
/// - `INVALID_REQUEST` if the `session_id` param doesn't match the
///   currently-active session (prevents clients from archiving someone
///   else's session by mistake)
pub(crate) async fn handle_session_archive(
    params: coco_types::SessionArchiveParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let session_id = params.target.session_id().clone();
    let Some(target_session_id) = ctx.target_session_id.as_ref() else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_PARAMS,
            message: "session/archive requires an explicit target".into(),
            data: Some(serde_json::json!({ "kind": "missing_session_target" })),
        };
    };
    if target_session_id != &session_id {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!(
                "session_id mismatch: target is {target_session_id}, archive requested for {session_id}"
            ),
            data: None,
        };
    }
    let Some(runtime) = ctx.resolve_runtime().await else {
        return HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: format!("session {session_id} has no live runtime"),
            data: Some(serde_json::json!({ "kind": "session_runtime_not_found" })),
        };
    };
    ctx.state.forget_session_activity(&session_id);
    info!(session_id = %session_id, "AppServerHost: session/archive");
    let archived = crate::session_archive::archive_live_session(
        &runtime,
        ctx.state.session_manager_snapshot().await,
        std::time::Duration::from_secs(5),
    )
    .await;

    // Emit the aggregated SessionResult on the outbound notification
    // channel. Ignore a send error (transport may have shut down)
    // since the state is already cleared.
    let result_event = CoreEvent::Protocol(coco_types::ServerNotification::SessionResult(
        Box::new(archived.result),
    ));
    let _ = send_session_event_and_wait(&ctx.notif_tx, session_id, result_event).await;

    HandlerResult::ok_empty()
}
