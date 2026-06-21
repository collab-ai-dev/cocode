//! MCP lifecycle handlers — `mcp/status` / `mcp/setServers` /
//! `mcp/reconnect` / `mcp/toggle`.
//!
//! All reachable routes require an [`coco_mcp::McpConnectionManager`] to
//! be wired via `SdkServer::with_mcp_manager`; otherwise they return
//! `INVALID_REQUEST`.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::info;

use super::HandlerContext;
use super::HandlerResult;

/// `mcp/status` — report MCP server connection status.
///
/// If an `McpConnectionManager` is wired, returns the actual connection
/// state for every registered server. Otherwise returns an empty list
/// (persistence disabled).
pub(super) async fn handle_mcp_status(ctx: &HandlerContext) -> HandlerResult {
    let manager = {
        let manager_slot = ctx.state.mcp_manager.read().await;
        let Some(manager) = manager_slot.as_ref() else {
            info!("SdkServer: mcp/status (no MCP manager wired, returning empty)");
            return HandlerResult::ok(coco_types::McpStatusResult {
                mcp_servers: Vec::new(),
            });
        };
        let manager = manager.lock().await;
        manager.clone()
    };
    let names = manager.registered_server_names();
    let reports = ctx.state.mcp_registration_reports.read().await;
    let mut statuses: Vec<coco_types::McpServerStatus> = Vec::new();
    for name in &names {
        let state = manager.get_state(name).await;
        let (status, error, advertised) = match state {
            Some(coco_mcp::McpConnectionState::Connected(server)) => (
                coco_types::McpConnectionStatus::Connected,
                None,
                server.tools.len() as i32,
            ),
            Some(coco_mcp::McpConnectionState::Pending { .. }) => {
                (coco_types::McpConnectionStatus::Pending, None, 0)
            }
            Some(coco_mcp::McpConnectionState::Failed { error }) => {
                (coco_types::McpConnectionStatus::Failed, Some(error), 0)
            }
            Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
                (coco_types::McpConnectionStatus::NeedsAuth, None, 0)
            }
            Some(coco_mcp::McpConnectionState::Disabled) => {
                (coco_types::McpConnectionStatus::Disabled, None, 0)
            }
            None => (coco_types::McpConnectionStatus::Disconnected, None, 0),
        };
        // v4.2: prefer the **registered** count (what the model can call) from
        // the last registration report; fall back to the advertised count when
        // no report exists (e.g. agent-inline connects bypassing the SDK path).
        let report = reports.get(name);
        let tool_count = report
            .map(|r| r.registered.len() as i32)
            .unwrap_or(advertised);
        let (skipped_tools, tombstoned_tools) = report
            .map(|r| {
                let skipped = r
                    .skipped
                    .iter()
                    .map(|s| coco_types::McpSkippedToolStatus {
                        tool_name: s.tool_name.clone(),
                        error: s.error.to_string(),
                    })
                    .collect();
                let tombstoned = r
                    .tombstones
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                (skipped, tombstoned)
            })
            .unwrap_or_default();
        statuses.push(coco_types::McpServerStatus {
            name: name.clone(),
            status,
            tool_count,
            error,
            skipped_tools,
            tombstoned_tools,
        });
    }
    drop(reports);
    info!(server_count = statuses.len(), "SdkServer: mcp/status");
    HandlerResult::ok(coco_types::McpStatusResult {
        mcp_servers: statuses,
    })
}

/// Convert the tools of a connected MCP server into `McpToolSchema` values
/// ready for `register_mcp_tools`.
///
/// Reads connected-state from the manager. Cloned managers share connection
/// state, so callers can avoid holding the SDK server's outer manager mutex
/// while waiting on MCP connection locks.
async fn collect_server_schemas(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Vec<coco_tool_runtime::McpToolSchema> {
    let Some(coco_mcp::McpConnectionState::Connected(server)) =
        manager.get_state(server_name).await
    else {
        return vec![];
    };
    server
        .tools
        .iter()
        .map(|t| coco_tool_runtime::McpToolSchema {
            server_name: server_name.to_string(),
            tool_name: t.name.clone(),
            description: t.description.clone(),
            annotations: coco_tool_runtime::McpToolAnnotations::from_input_schema_meta(
                &t.input_schema,
            ),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

pub(crate) async fn collect_server_schemas_for_manager(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Vec<coco_tool_runtime::McpToolSchema> {
    collect_server_schemas(manager, server_name).await
}

/// Register MCP server tools in the shared `ToolRegistry` via the session
/// runtime, if one is currently wired.
async fn register_server_tools(
    ctx: &HandlerContext,
    server_name: &str,
    schemas: Vec<coco_tool_runtime::McpToolSchema>,
) {
    // Register under the session-runtime lock (sync), then persist the report
    // after releasing it (the report store is a separate lock).
    let report = {
        let rt_guard = ctx.state.session_runtime.read().await;
        let Some(rt) = rt_guard.as_ref() else { return };
        coco_tools::register_mcp_tools(rt.tools(), server_name, schemas)
    };
    ctx.state
        .record_mcp_registration_report(server_name, report)
        .await;
}

/// Surface the per-server `mcp__<server>__authenticate` pseudo-tool for a
/// NeedsAuth server in the shared `ToolRegistry`. Mirrors
/// [`register_server_tools`] but registers the auth affordance instead
/// of real tools.
async fn register_server_auth_tool(
    ctx: &HandlerContext,
    server_name: &str,
    transport: &str,
    url: Option<&str>,
) {
    let rt_guard = ctx.state.session_runtime.read().await;
    let Some(rt) = rt_guard.as_ref() else { return };
    coco_tools::register_mcp_auth_tool(rt.tools(), server_name, transport, url);
}

/// After a connect that returned `Err`, classify whether the server actually
/// landed in `NeedsAuth` (a 401 for lack of credentials). Returns the
/// transport+url descriptor for surfacing the authenticate tool, or `None` for
/// a genuine failure. Pure read — the caller drops the manager lock before
/// registering (avoids holding it across the session-runtime lock).
async fn needs_auth_descriptor(
    manager: &coco_mcp::McpConnectionManager,
    server_name: &str,
) -> Option<(String, Option<String>)> {
    match manager.get_state(server_name).await {
        Some(coco_mcp::McpConnectionState::NeedsAuth { .. }) => {
            manager.auth_descriptor(server_name)
        }
        _ => None,
    }
}

/// Deregister all tools for an MCP server from the shared `ToolRegistry`.
async fn deregister_server_tools(ctx: &HandlerContext, server_name: &str) {
    {
        let rt_guard = ctx.state.session_runtime.read().await;
        if let Some(rt) = rt_guard.as_ref() {
            coco_tools::deregister_mcp_server(rt.tools(), server_name);
        }
    }
    ctx.state.clear_mcp_registration_report(server_name).await;
}

/// `SendElicitation` factory for SDK-driven MCP connects.
///
/// The base closure bridges MCP server-initiated elicitations to the
/// connected SDK client via the SDK control protocol
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
    build_send_elicitation_for_state(ctx.state.clone(), server_name.to_string()).await
}

pub(crate) async fn build_send_elicitation_for_state(
    state: Arc<super::SdkServerState>,
    server_name: String,
) -> coco_mcp::SendElicitation {
    use std::future::Future;
    use std::pin::Pin;
    let server_name_for_base = server_name.clone();
    let base_state = state.clone();
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
            let state = base_state.clone();
            let server_name = server_name_for_base.clone();
            Box::pin(async move {
                bridge_elicitation_to_sdk_client(&state, &server_name, elicitation).await
            })
        },
    );
    let runtime = {
        let guard = state.session_runtime.read().await;
        guard.clone()
    };
    let Some(runtime) = runtime else { return base };
    let registry = runtime.hook_registry.clone();
    let factory = runtime.orchestration_ctx_factory();
    // Phase 7: pull the elicitation counter Arc so the wrapper holds
    // an `ElicitationGuard` for each in-flight request — drives
    // `prompt_suggestion::SuppressReason::ElicitationActive`.
    let elicit_counter = runtime
        .app_state
        .read()
        .await
        .elicitation_pending_count
        .clone();
    crate::elicitation_hooks::wrap_send_elicitation_with_hooks(
        server_name,
        registry,
        factory,
        Some(elicit_counter),
        base,
    )
}

/// Bridge a single MCP-server-initiated elicitation to the SDK client.
///
/// Allocates a fresh `request_id`, serializes the rmcp `Elicitation`
/// payload, sends a `ServerRequest::RequestElicitation` via the
/// transport, awaits the client's response, and maps the result back to
/// the rmcp [`coco_mcp::ElicitationResponse`] shape. The SDK client is
/// the ultimate authority for MCP elicitations in SDK mode.
async fn bridge_elicitation_to_sdk_client(
    state: &Arc<super::SdkServerState>,
    server_name: &str,
    elicitation: impl serde::Serialize,
) -> std::result::Result<coco_mcp::ElicitationResponse, coco_mcp::RmcpClientError> {
    // Grab the cached transport handle — must be present (dispatcher
    // publishes it at `SdkServer::run` startup).
    let transport = {
        let guard = state.transport.read().await;
        match guard.as_ref() {
            Some(t) => t.clone(),
            None => {
                return Err(coco_mcp::RmcpClientError::generic(
                    "elicitation bridge: SDK transport not initialized yet",
                ));
            }
        }
    };

    let request_id = uuid::Uuid::new_v4().to_string();
    let elicitation_json = serde_json::to_value(&elicitation).map_err(|e| {
        coco_mcp::RmcpClientError::generic(format!("serialize elicitation payload: {e}"))
    })?;
    let params = coco_types::ServerRequestElicitationParams {
        request_id: request_id.clone(),
        mcp_server_name: server_name.to_string(),
        elicitation: elicitation_json,
    };
    let params_json = serde_json::to_value(&params).map_err(|e| {
        coco_mcp::RmcpClientError::generic(format!("serialize RequestElicitationParams: {e}"))
    })?;

    let reply = state
        .send_server_request(&transport, "mcp/requestElicitation", params_json)
        .await
        .map_err(|e| {
            coco_mcp::RmcpClientError::generic(format!("send mcp/requestElicitation: {e}"))
        })?;

    let resolved: coco_types::ElicitationResolveParams = match reply {
        coco_types::JsonRpcMessage::Response(r) => serde_json::from_value(r.result)
            .map_err(|e| coco_mcp::RmcpClientError::generic(format!("parse SDK reply: {e}")))?,
        coco_types::JsonRpcMessage::Error(e) => {
            return Err(coco_mcp::RmcpClientError::generic(format!(
                "SDK client returned error for mcp/requestElicitation: {} ({})",
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

/// Helper: borrow the wired MCP manager or return INVALID_REQUEST.
async fn require_mcp_manager(
    ctx: &HandlerContext,
) -> Result<Arc<Mutex<coco_mcp::McpConnectionManager>>, HandlerResult> {
    let slot = ctx.state.mcp_manager.read().await;
    match slot.as_ref() {
        Some(m) => Ok(m.clone()),
        None => Err(HandlerResult::Err {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "MCP manager not enabled on this server".into(),
            data: None,
        }),
    }
}

/// `mcp/setServers` — register or replace MCP server configurations.
///
/// For each `(name, config_json)` pair in `params.servers`, this
/// handler:
/// 1. Deserializes the JSON value into [`coco_mcp::McpServerConfig`]
///    (transport-tagged enum).
/// 2. Wraps it in a [`coco_mcp::ScopedMcpServerConfig`] with
///    `scope = ConfigScope::Dynamic` and no plugin source.
/// 3. Calls `register_server(...)` on the live manager.
///
/// Note that this only **registers** the configs — it does not
/// auto-connect. Use `mcp/reconnect` (or the existing tool layer's
/// connect-on-first-use logic) to actually establish connections.
///
/// Returns:
/// - `added`: names that were added or replaced
/// - `removed`: always empty in this implementation (no diff vs prior state)
/// - `errors`: per-name deserialization errors
pub(super) async fn handle_mcp_set_servers(
    params: coco_types::McpSetServersParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let manager_arc = match require_mcp_manager(ctx).await {
        Ok(m) => m,
        Err(e) => return e,
    };
    let mut manager = manager_arc.lock().await;
    let mut added: Vec<String> = Vec::new();
    let mut errors: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, config_json) in params.servers {
        match serde_json::from_value::<coco_mcp::McpServerConfig>(config_json) {
            Ok(config) => {
                let scoped = coco_mcp::ScopedMcpServerConfig {
                    name: name.clone(),
                    config,
                    scope: coco_mcp::ConfigScope::Dynamic,
                    plugin_source: None,
                };
                manager.register_server(scoped);
                added.push(name);
            }
            Err(e) => {
                errors.insert(name, format!("invalid mcp config: {e}"));
            }
        }
    }
    info!(
        added = added.len(),
        errors = errors.len(),
        "SdkServer: mcp/setServers"
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
pub(super) async fn handle_mcp_reconnect(
    params: coco_types::McpReconnectParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let manager_arc = match require_mcp_manager(ctx).await {
        Ok(m) => m,
        Err(e) => return e,
    };
    let send_elicitation = build_send_elicitation(ctx, &params.server_name).await;
    let manager = manager_arc.lock().await;
    manager.disconnect(&params.server_name).await;
    match manager.connect(&params.server_name, send_elicitation).await {
        Ok(()) => {
            info!(server = %params.server_name, "SdkServer: mcp/reconnect ok");
            let schemas = collect_server_schemas(&manager, &params.server_name).await;
            drop(manager);
            register_server_tools(ctx, &params.server_name, schemas).await;
            HandlerResult::ok_empty()
        }
        Err(e) => {
            // A connect `Err` may actually be NeedsAuth (401 for lack of
            // credentials). Surface the authenticate tool and answer ok rather
            // than failing the control request.
            let descriptor = needs_auth_descriptor(&manager, &params.server_name).await;
            drop(manager);
            if let Some((transport, url)) = descriptor {
                register_server_auth_tool(ctx, &params.server_name, &transport, url.as_deref())
                    .await;
                info!(server = %params.server_name, %transport, "SdkServer: mcp/reconnect needs auth; surfaced authenticate tool");
                HandlerResult::ok_empty()
            } else {
                HandlerResult::Err {
                    code: coco_types::error_codes::INTERNAL_ERROR,
                    message: format!("mcp/reconnect: {e}"),
                    data: None,
                }
            }
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
pub(super) async fn handle_mcp_toggle(
    params: coco_types::McpToggleParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let manager_arc = match require_mcp_manager(ctx).await {
        Ok(m) => m,
        Err(e) => return e,
    };
    let send_elicitation = build_send_elicitation(ctx, &params.server_name).await;
    let manager = manager_arc.lock().await;
    if params.enabled {
        match manager.connect(&params.server_name, send_elicitation).await {
            Ok(()) => {
                info!(server = %params.server_name, "SdkServer: mcp/toggle (enabled)");
                let schemas = collect_server_schemas(&manager, &params.server_name).await;
                drop(manager);
                register_server_tools(ctx, &params.server_name, schemas).await;
                HandlerResult::ok_empty()
            }
            Err(e) => {
                let descriptor = needs_auth_descriptor(&manager, &params.server_name).await;
                drop(manager);
                if let Some((transport, url)) = descriptor {
                    register_server_auth_tool(ctx, &params.server_name, &transport, url.as_deref())
                        .await;
                    info!(server = %params.server_name, %transport, "SdkServer: mcp/toggle needs auth; surfaced authenticate tool");
                    HandlerResult::ok_empty()
                } else {
                    HandlerResult::Err {
                        code: coco_types::error_codes::INTERNAL_ERROR,
                        message: format!("mcp/toggle enable: {e}"),
                        data: None,
                    }
                }
            }
        }
    } else {
        manager.disconnect(&params.server_name).await;
        info!(server = %params.server_name, "SdkServer: mcp/toggle (disabled)");
        drop(manager);
        deregister_server_tools(ctx, &params.server_name).await;
        HandlerResult::ok_empty()
    }
}
