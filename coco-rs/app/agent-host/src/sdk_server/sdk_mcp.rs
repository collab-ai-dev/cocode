//! SDK-hosted MCP bridge.
//!
//! SDK MCP servers live in the SDK client process. The Rust MCP manager
//! owns lifecycle/tool catalog state and forwards MCP JSON-RPC messages
//! through `mcp/routeMessage` server requests.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::warn;

pub async fn install_route(
    manager: Arc<Mutex<coco_mcp::McpConnectionManager>>,
    app_server: Arc<coco_app_server::AppServer<super::LocalAppSessionHandle>>,
    session_id: coco_types::SessionId,
) {
    let route = Arc::new(
        move |server_name: String, message: serde_json::Value| -> coco_mcp::SdkRouteFuture {
            let app_server = Arc::clone(&app_server);
            let session_id = session_id.clone();
            Box::pin(
                async move { route_message(app_server, session_id, server_name, message).await },
            )
        },
    );
    manager.lock().await.set_sdk_route_message(route);
}

pub async fn register_and_connect(
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<coco_app_server::AppServer<super::LocalAppSessionHandle>>,
    server_names: Vec<String>,
) -> Result<(), String> {
    let manager = session
        .mcp_manager()
        .await
        .ok_or_else(|| "MCP manager not enabled".to_string())?;
    install_route(
        manager.clone(),
        Arc::clone(&app_server),
        session.session_id().clone(),
    )
    .await;
    {
        let mut manager_guard = manager.lock().await;
        for name in &server_names {
            manager_guard.register_server(coco_mcp::ScopedMcpServerConfig {
                name: name.clone(),
                config: coco_mcp::McpServerConfig::Sdk(coco_mcp::types::McpSdkConfig {
                    name: name.clone(),
                }),
                scope: coco_mcp::ConfigScope::Dynamic,
                plugin_source: None,
            });
        }
    }

    for name in server_names {
        let send_elicitation =
            crate::sdk_server::handlers::mcp::build_send_elicitation_for_session(
                session.clone(),
                Arc::clone(&app_server),
                name.clone(),
            )
            .await;
        let manager_for_connect = {
            let manager_guard = manager.lock().await;
            manager_guard.clone()
        };
        if let Err(error) = manager_for_connect.connect(&name, send_elicitation).await {
            warn!(
                session_id = %session.session_id(),
                server = %name,
                error = %error,
                "SDK MCP connect failed"
            );
            continue;
        }
        let schemas = crate::sdk_server::handlers::mcp::collect_server_schemas_for_manager(
            &manager_for_connect,
            &name,
        )
        .await;
        let report = coco_tools::register_mcp_tools(session.tools(), &name, schemas);
        session.record_mcp_registration_report(&name, report).await;
    }
    Ok(())
}

async fn route_message(
    app_server: Arc<coco_app_server::AppServer<super::LocalAppSessionHandle>>,
    session_id: coco_types::SessionId,
    server_name: String,
    message: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let params = coco_types::ServerMcpRouteMessageParams {
        server_name,
        message,
    };
    let reply = app_server
        .route_server_request_with_reply(
            session_id,
            coco_app_server::SurfaceCapability::Interactive,
            None,
            coco_types::ServerRequest::McpRouteMessage(params),
        )
        .map_err(|error| format!("route mcp/routeMessage: {error:?}"))?
        .await
        .map_err(|_| "mcp/routeMessage reply channel closed".to_string())?;
    match reply {
        coco_app_server::ServerRequestReply::McpRouteMessage { result, .. } => {
            // The SDK's reply body is `{message: <raw JSON-RPC message
            // from the SDK-hosted MCP server>}`. Typed parse so a
            // malformed body errors here rather than downstream.
            let resolved: coco_types::McpRouteMessageResult = serde_json::from_value(result)
                .map_err(|e| format!("parse mcp/routeMessage response: {e}"))?;
            Ok(resolved.message)
        }
        coco_app_server::ServerRequestReply::Error(error) => Err(format!(
            "SDK client returned mcp/routeMessage error: {} ({})",
            error.message, error.code
        )),
        other => Err(format!("unexpected mcp/routeMessage reply: {other:?}")),
    }
}
