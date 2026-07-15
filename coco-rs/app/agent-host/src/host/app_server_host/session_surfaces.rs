use std::sync::Arc;

use coco_app_server::{
    AppServer, AttachSurfaceOptions, ConnectionKey, JsonRpcDispatchError, SubscribeReplay,
    SurfaceRole,
};
use coco_types::{CoreEvent, SessionEnvelope, SessionId, SurfaceId};

use crate::app_session::AppSessionHandle;

use super::session_errors::{LifecycleError, attach_lifecycle_error_parts};

pub(crate) fn attach_local_app_server_surface(
    app_server: &Arc<AppServer<AppSessionHandle>>,
    connection: ConnectionKey,
    session_id: SessionId,
) -> Result<SurfaceId, LifecycleError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..Default::default()
    };
    app_server
        .attach_live_surface_with_options(connection, surface_id.clone(), session_id, options)
        .map_err(|error| attach_lifecycle_error_parts("attach session surface", error))?;
    Ok(surface_id)
}

pub(crate) async fn subscribe_local_app_server_session(
    app_server: Arc<AppServer<AppSessionHandle>>,
    connection: ConnectionKey,
    params: coco_types::SessionSubscribeParams,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    let surface_id = SurfaceId::generate();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Passive,
        ..Default::default()
    };
    match app_server
        .subscribe_live_surface_with_options(
            connection,
            surface_id.clone(),
            params.target.session_id.clone(),
            params.after_seq,
            options,
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
                surface_id,
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
            message: "session/subscribe requires a fresh snapshot before passive attach"
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

pub(crate) fn local_replace_calling_surface(
    app_server: &AppServer<AppSessionHandle>,
    session_id: &SessionId,
) -> Option<SurfaceId> {
    let routing = app_server
        .routing()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    routing.interactive_owner(session_id).cloned()
}
