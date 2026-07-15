//! Client-hosted MCP bridge.
//!
//! Client-hosted MCP servers live in the remote client process. The Rust MCP
//! manager owns lifecycle/tool catalog state and forwards MCP JSON-RPC
//! messages through `mcp/routeMessage` server requests.

use std::sync::Arc;

use tracing::warn;

fn build_route(
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    session_id: coco_types::SessionId,
) -> coco_mcp::ClientRouteMessage {
    Arc::new(
        move |server_name: String, message: serde_json::Value| -> coco_mcp::ClientRouteFuture {
            let app_server = Arc::clone(&app_server);
            let session_id = session_id.clone();
            Box::pin(
                async move { route_message(app_server, session_id, server_name, message).await },
            )
        },
    )
}

pub async fn register_and_connect(
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    server_names: Vec<String>,
) -> Result<(), String> {
    let route = build_route(Arc::clone(&app_server), session.session_id().clone());
    if !session.install_client_mcp_route(route).await {
        return Err("MCP manager not enabled".to_string());
    }
    if !session.register_client_mcp_servers(&server_names).await {
        return Err("MCP manager not enabled".to_string());
    }

    for name in server_names {
        let send_elicitation =
            crate::app_server_host::request_handlers::mcp::build_send_elicitation_for_session(
                session.clone(),
                Arc::clone(&app_server),
                name.clone(),
            )
            .await;
        let change = crate::session_mcp::set_enabled(
            Some(session.clone()),
            &name,
            true,
            Some(send_elicitation),
        )
        .await
        .map_err(|error| error.to_string())?;
        match change {
            crate::session_runtime::SessionMcpConnectionChange::Connected => {}
            crate::session_runtime::SessionMcpConnectionChange::NeedsAuth { transport, .. } => {
                warn!(
                    session_id = %session.session_id(),
                    server = %name,
                    %transport,
                    "client-hosted MCP connect needs auth"
                );
            }
            crate::session_runtime::SessionMcpConnectionChange::Failed(error) => {
                warn!(
                    session_id = %session.session_id(),
                    server = %name,
                    error = %error,
                    "client-hosted MCP connect failed"
                );
            }
            crate::session_runtime::SessionMcpConnectionChange::Disconnected => {}
        }
    }
    Ok(())
}

async fn route_message(
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    session_id: coco_types::SessionId,
    server_name: String,
    message: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let params = coco_types::ServerMcpRouteMessageParams {
        server_name,
        message,
    };
    // MCP route requests may fire inside a turn (tool-driven) or outside one
    // (connection setup); tag with the active turn if there is one so the
    // request is cancelled when that turn ends.
    let turn_id = app_server
        .registry()
        .get(&session_id)
        .and_then(|handle| handle.into_session().active_turn_id());
    let reply = app_server
        .route_server_request_with_reply(
            session_id,
            coco_app_server::SurfaceCapability::Interactive,
            turn_id,
            coco_types::ServerRequest::McpRouteMessage(params),
        )
        .map_err(|error| format!("route mcp/routeMessage: {error:?}"))?
        .await
        .map_err(|_| "mcp/routeMessage reply channel closed".to_string())?;
    match reply {
        coco_app_server::ServerRequestReply::McpRouteMessage { result, .. } => {
            // The remote client's reply body is `{message: <raw JSON-RPC message
            // from the client-hosted MCP server>}`. Typed parse so a
            // malformed body errors here rather than downstream.
            let resolved: coco_types::McpRouteMessageResult = serde_json::from_value(result)
                .map_err(|e| format!("parse mcp/routeMessage response: {e}"))?;
            Ok(resolved.message)
        }
        coco_app_server::ServerRequestReply::Error(error) => Err(format!(
            "remote client returned mcp/routeMessage error: {} ({})",
            error.message, error.code
        )),
        other => Err(format!("unexpected mcp/routeMessage reply: {other:?}")),
    }
}
