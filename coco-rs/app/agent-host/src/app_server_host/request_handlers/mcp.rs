//! MCP lifecycle handlers — `mcp/status` / `mcp/setServers` /
//! `mcp/reconnect` / `mcp/toggle`.
//!
//! Handlers own DTO parsing and JSON-RPC error mapping; live MCP state is
//! accessed through the targeted session handle.

use std::sync::Arc;

use tracing::info;

use super::{HandlerContext, HandlerResult};
use crate::session_mcp::{self, SessionMcpError};

/// `mcp/status` — report MCP server connection status.
///
/// If the targeted session has an MCP manager, returns the actual connection
/// state for every registered server. Otherwise returns an empty list.
pub(crate) async fn handle_mcp_status(ctx: &HandlerContext) -> HandlerResult {
    let result = match session_mcp::status(ctx.resolve_runtime().await).await {
        Ok(result) => result,
        Err(error) => return mcp_error(error),
    };
    if result.mcp_servers.is_empty() {
        info!("AppServerHost: mcp/status (no MCP manager wired, returning empty)");
    }
    info!(
        server_count = result.mcp_servers.len(),
        "AppServerHost: mcp/status"
    );
    HandlerResult::ok(result)
}

/// `SendElicitation` factory for AppServer-routed MCP connects.
///
/// The base closure bridges MCP server-initiated elicitations to the
/// connected interactive surface via AppServer server requests
/// (`ServerRequest::RequestElicitation` →
/// `ClientRequest::ElicitationResolve` synchronous reply). When a
/// session runtime with a hook registry is wired, we wrap the closure
/// so `Elicitation` / `ElicitationResult` hooks fire first — a hook
/// can program-respond with accept/decline and short-circuit the
/// bridge entirely.
async fn build_send_elicitation(
    ctx: &HandlerContext,
    server_name: &str,
) -> coco_mcp::SendElicitation {
    let Some(session) = ctx.resolve_runtime().await else {
        return unavailable_elicitation_bridge();
    };
    let Some(app_server) = ctx.app_server.clone() else {
        return unavailable_elicitation_bridge();
    };
    build_send_elicitation_for_session(session, app_server, server_name.to_string()).await
}

pub(crate) async fn build_send_elicitation_for_session(
    session: crate::session_runtime::SessionHandle,
    app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    server_name: String,
) -> coco_mcp::SendElicitation {
    use std::{future::Future, pin::Pin};
    let server_name_for_base = server_name.clone();
    let base_server = Arc::clone(&app_server);
    let base_session_id = session.session_id().clone();
    let base: coco_mcp::SendElicitation = Box::new(
        move |_request_id,
              elicitation|
              -> Pin<
            Box<
                dyn Future<
                        Output = std::result::Result<
                            coco_mcp::ElicitationResponse,
                            coco_mcp::RmcpClientError,
                        >,
                    > + Send,
            >,
        > {
            let app_server = Arc::clone(&base_server);
            let session_id = base_session_id.clone();
            let server_name = server_name_for_base.clone();
            Box::pin(async move {
                bridge_elicitation_to_remote_surface(
                    &app_server,
                    session_id,
                    &server_name,
                    elicitation,
                )
                .await
            })
        },
    );
    session_mcp::wrap_send_elicitation_with_hooks(&session, server_name, base).await
}

fn unavailable_elicitation_bridge() -> coco_mcp::SendElicitation {
    Box::new(|_, _| {
        Box::pin(async {
            Err(coco_mcp::RmcpClientError::generic(
                "elicitation requires a live targeted AppServer session",
            ))
        })
    })
}

/// Bridge a single MCP-server-initiated elicitation to the remote surface.
///
/// Allocates a fresh `request_id`, serializes the rmcp `Elicitation`
/// payload, sends a `ServerRequest::RequestElicitation` via the
/// AppServer route, awaits the surface response, and maps the result back
/// to the rmcp [`coco_mcp::ElicitationResponse`] shape.
async fn bridge_elicitation_to_remote_surface(
    app_server: &Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
    session_id: coco_types::SessionId,
    server_name: &str,
    elicitation: impl serde::Serialize,
) -> std::result::Result<coco_mcp::ElicitationResponse, coco_mcp::RmcpClientError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let elicitation_json = serde_json::to_value(&elicitation).map_err(|e| {
        coco_mcp::RmcpClientError::generic(format!("serialize elicitation payload: {e}"))
    })?;
    let params = coco_types::ServerRequestElicitationParams {
        request_id: request_id.clone(),
        mcp_server_name: server_name.to_string(),
        elicitation: elicitation_json,
    };
    let reply = app_server
        .route_server_request_with_reply(
            session_id,
            coco_app_server::SurfaceCapability::Interactive,
            None,
            coco_types::ServerRequest::RequestElicitation(params),
        )
        .map_err(|error| {
            coco_mcp::RmcpClientError::generic(format!("route elicitation: {error:?}"))
        })?
        .await
        .map_err(|_| coco_mcp::RmcpClientError::generic("elicitation reply channel closed"))?;

    let resolved = match reply {
        coco_app_server::ServerRequestReply::Elicitation(resolved) => resolved,
        coco_app_server::ServerRequestReply::Error(e) => {
            return Err(coco_mcp::RmcpClientError::generic(format!(
                "remote surface returned error for mcp/requestElicitation: {} ({})",
                e.message, e.code
            )));
        }
        other => {
            return Err(coco_mcp::RmcpClientError::generic(format!(
                "unexpected reply variant for mcp/requestElicitation: {other:?}"
            )));
        }
    };

    let action = if resolved.approved {
        coco_mcp::RmcpElicitationAction::Accept
    } else {
        coco_mcp::RmcpElicitationAction::Decline
    };
    let content = if resolved.approved && !resolved.values.is_empty() {
        Some(serde_json::Value::Object(
            resolved.values.into_iter().collect(),
        ))
    } else {
        None
    };
    Ok(coco_mcp::ElicitationResponse {
        action,
        content,
        meta: None,
    })
}

fn mcp_error(error: SessionMcpError) -> HandlerResult {
    let code = match error {
        SessionMcpError::LiveSessionRequired { .. }
        | SessionMcpError::ManagerNotEnabled
        | SessionMcpError::ElicitationRequired => coco_types::error_codes::INVALID_REQUEST,
    };
    HandlerResult::Err {
        code,
        message: error.to_string(),
        data: None,
    }
}

/// `mcp/setServers` — register or replace MCP server configurations.
///
/// For each `(name, config_json)` pair in `params.servers`, this
/// handler:
/// 1. Deserializes the JSON value into [`coco_mcp::McpServerConfig`]
///    (transport-tagged enum).
/// 2. Passes the parsed configs to the targeted session for dynamic
///    registration.
///
/// Note that this only **registers** the configs — it does not
/// auto-connect. Use `mcp/reconnect` (or the existing tool layer's
/// connect-on-first-use logic) to actually establish connections.
///
/// Returns:
/// - `added`: names that were added or replaced
/// - `removed`: always empty in this implementation (no diff vs prior state)
/// - `errors`: per-name deserialization errors
pub(crate) async fn handle_mcp_set_servers(
    params: coco_types::McpSetServersParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let mut parsed = Vec::new();
    let mut errors: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, config_json) in params.servers {
        match serde_json::from_value::<coco_mcp::McpServerConfig>(config_json) {
            Ok(config) => {
                parsed.push((name, config));
            }
            Err(e) => {
                errors.insert(name, format!("invalid mcp config: {e}"));
            }
        }
    }
    let added = match session_mcp::set_dynamic_servers(ctx.resolve_runtime().await, parsed).await {
        Ok(added) => added,
        Err(error) => return mcp_error(error),
    };
    info!(
        added = added.len(),
        errors = errors.len(),
        "AppServerHost: mcp/setServers"
    );
    HandlerResult::ok(coco_types::McpSetServersResult {
        added,
        removed: Vec::new(),
        errors,
    })
}

/// `mcp/reconnect` — disconnect + reconnect a specific MCP server.
///
/// Useful after a server's process has been restarted externally or
/// after a transient network failure. The handler unconditionally
/// disconnects (no-op if not connected) then attempts to connect
/// using a no-op elicitation callback.
///
/// Errors:
/// - `INVALID_REQUEST` if MCP manager not enabled
/// - `INTERNAL_ERROR` if the connect attempt fails (e.g. server
///   process refused, OAuth required without elicitation bridge)
pub(crate) async fn handle_mcp_reconnect(
    params: coco_types::McpReconnectParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let send_elicitation = build_send_elicitation(ctx, &params.server_name).await;
    let change = match session_mcp::reconnect(
        ctx.resolve_runtime().await,
        &params.server_name,
        send_elicitation,
    )
    .await
    {
        Ok(change) => change,
        Err(error) => return mcp_error(error),
    };
    match change {
        crate::session_runtime::SessionMcpConnectionChange::Connected => {
            info!(server = %params.server_name, "AppServerHost: mcp/reconnect ok");
            HandlerResult::ok_empty()
        }
        crate::session_runtime::SessionMcpConnectionChange::NeedsAuth { transport, .. } => {
            info!(server = %params.server_name, %transport, "AppServerHost: mcp/reconnect needs auth; surfaced authenticate tool");
            HandlerResult::ok_empty()
        }
        crate::session_runtime::SessionMcpConnectionChange::Failed(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("mcp/reconnect: {error}"),
            data: None,
        },
        crate::session_runtime::SessionMcpConnectionChange::Disconnected => {
            HandlerResult::ok_empty()
        }
    }
}

/// `mcp/toggle` — enable or disable an MCP server.
///
/// `enabled = true`: ensures the server is connected (no-op if
/// already connected).
/// `enabled = false`: disconnects the server.
///
/// Errors:
/// - `INVALID_REQUEST` if MCP manager not enabled
/// - `INTERNAL_ERROR` if enabling and the connect attempt fails
pub(crate) async fn handle_mcp_toggle(
    params: coco_types::McpToggleParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let send_elicitation = if params.enabled {
        Some(build_send_elicitation(ctx, &params.server_name).await)
    } else {
        None
    };
    let change = match session_mcp::set_enabled(
        ctx.resolve_runtime().await,
        &params.server_name,
        params.enabled,
        send_elicitation,
    )
    .await
    {
        Ok(change) => change,
        Err(error) => return mcp_error(error),
    };
    match change {
        crate::session_runtime::SessionMcpConnectionChange::Connected => {
            info!(server = %params.server_name, "AppServerHost: mcp/toggle (enabled)");
            HandlerResult::ok_empty()
        }
        crate::session_runtime::SessionMcpConnectionChange::NeedsAuth { transport, .. } => {
            info!(server = %params.server_name, %transport, "AppServerHost: mcp/toggle needs auth; surfaced authenticate tool");
            HandlerResult::ok_empty()
        }
        crate::session_runtime::SessionMcpConnectionChange::Disconnected => {
            info!(server = %params.server_name, "AppServerHost: mcp/toggle (disabled)");
            HandlerResult::ok_empty()
        }
        crate::session_runtime::SessionMcpConnectionChange::Failed(error) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("mcp/toggle enable: {error}"),
            data: None,
        },
    }
}
