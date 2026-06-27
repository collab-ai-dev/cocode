//! SDK-hosted MCP bridge.
//!
//! SDK MCP servers live in the SDK client process. The Rust MCP manager
//! owns lifecycle/tool catalog state and forwards MCP JSON-RPC messages
//! through `mcp/routeMessage` server requests.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::warn;

use crate::sdk_server::handlers::SdkServerState;
use crate::sdk_server::transport::SdkTransport;

pub async fn install_route(
    manager: Arc<Mutex<coco_mcp::McpConnectionManager>>,
    state: Arc<SdkServerState>,
    transport: Arc<dyn SdkTransport>,
) {
    let route = Arc::new(
        move |server_name: String, message: serde_json::Value| -> coco_mcp::SdkRouteFuture {
            let state = state.clone();
            let transport = transport.clone();
            Box::pin(async move { route_message(state, transport, server_name, message).await })
        },
    );
    manager.lock().await.set_sdk_route_message(route);
}

pub async fn register_and_connect(
    state: Arc<SdkServerState>,
    server_names: Vec<String>,
) -> Result<(), String> {
    let manager = {
        let slot = state.mcp_manager.read().await;
        slot.as_ref()
            .cloned()
            .ok_or_else(|| "MCP manager not enabled".to_string())?
    };
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
        let send_elicitation = crate::sdk_server::handlers::mcp::build_send_elicitation_for_state(
            state.clone(),
            name.clone(),
        )
        .await;
        let manager_for_connect = {
            let manager_guard = manager.lock().await;
            manager_guard.clone()
        };
        if let Err(error) = manager_for_connect.connect(&name, send_elicitation).await {
            warn!(server = %name, error = %error, "SDK MCP connect failed");
            continue;
        }
        let schemas = crate::sdk_server::handlers::mcp::collect_server_schemas_for_manager(
            &manager_for_connect,
            &name,
        )
        .await;
        let report = {
            let guard = state.session_runtime.read().await;
            guard
                .as_ref()
                .map(|runtime| coco_tools::register_mcp_tools(runtime.tools(), &name, schemas))
        };
        if let Some(report) = report {
            state.record_mcp_registration_report(&name, report).await;
        }
    }
    Ok(())
}

async fn route_message(
    state: Arc<SdkServerState>,
    transport: Arc<dyn SdkTransport>,
    server_name: String,
    message: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let params = coco_types::ServerMcpRouteMessageParams {
        server_name,
        message,
    };
    let params_json = serde_json::to_value(params)
        .map_err(|e| format!("serialize mcp/routeMessage params: {e}"))?;
    let reply = state
        .send_server_request(&transport, "mcp/routeMessage", params_json)
        .await
        .map_err(|e| format!("send mcp/routeMessage: {e}"))?;
    match reply {
        coco_types::JsonRpcMessage::Response(response) => {
            // The SDK's reply body is `{message: <raw JSON-RPC message
            // from the SDK-hosted MCP server>}`. Typed parse so a
            // malformed body errors here rather than downstream.
            let resolved: coco_types::McpRouteMessageResult =
                serde_json::from_value(response.result)
                    .map_err(|e| format!("parse mcp/routeMessage response: {e}"))?;
            Ok(resolved.message)
        }
        coco_types::JsonRpcMessage::Error(error) => Err(format!(
            "SDK client returned mcp/routeMessage error: {} ({})",
            error.message, error.code
        )),
        other => Err(format!("unexpected mcp/routeMessage reply: {other:?}")),
    }
}
