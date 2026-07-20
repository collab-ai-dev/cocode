use std::sync::Arc;

use coco_app_server::{
    AppServer, AttachSessionOptions, ConnectionKey, JsonRpcDispatchError, SubscribeReplay,
};
use coco_types::{CoreEvent, SessionEnvelope, SessionId};

use crate::app_session::AppSessionHandle;

use super::session_errors::{LifecycleError, attach_lifecycle_error_parts};

pub(crate) fn attach_local_app_server_session(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    connection: ConnectionKey,
    session_id: SessionId,
) -> Result<(), LifecycleError> {
    app_server
        .attach_live_session(connection, session_id, AttachSessionOptions::full())
        .map_err(|error| attach_lifecycle_error_parts("attach session", error))
}

pub(crate) async fn subscribe_local_app_server_session(
    app_server: Arc<AppServer<AppSessionHandle>>,
    connection: ConnectionKey,
    params: coco_types::SessionSubscribeParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    match app_server
        .subscribe_live_session(
            connection,
            params.target.session_id.clone(),
            params.after_seq,
            AttachSessionOptions::read_only(),
        )
        .map_err(|error| {
            attach_lifecycle_error_parts("subscribe session", error).into_dispatch_error()
        })? {
        SubscribeReplay::Replayed(replayed) => {
            let replayed = replayed
                .into_iter()
                .map(encode_session_subscribe_envelope)
                .collect();
            serde_json::to_value(coco_types::SessionSubscribeResult {
                session_id: params.target.session_id,
                replayed,
            })
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/subscribe encode failed: {error}"),
                data: None,
            })
        }
        SubscribeReplay::SnapshotRequired => Err(JsonRpcDispatchError {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "session/subscribe requires a fresh snapshot before read-only attach"
                .to_string(),
            data: Some(serde_json::json!({ "kind": "snapshot_required" })),
        }),
    }
}

pub(crate) fn encode_session_subscribe_envelope(
    envelope: SessionEnvelope,
) -> coco_types::SessionSubscribeEnvelope {
    let event = match envelope.event {
        CoreEvent::Protocol(notification) => serde_json::json!({
            "layer": "protocol",
            "payload": notification,
        }),
        CoreEvent::Stream(event) => serde_json::json!({
            "layer": "stream",
            "payload": event,
        }),
        CoreEvent::Tui(event) => serde_json::json!({
            "layer": "tui",
            "payload": event,
        }),
    };
    coco_types::SessionSubscribeEnvelope {
        session_id: envelope.session_id,
        agent_id: envelope.agent_id.map(coco_types::AgentId::into_inner),
        turn_id: envelope.turn_id,
        session_seq: envelope.session_seq,
        event,
    }
}
