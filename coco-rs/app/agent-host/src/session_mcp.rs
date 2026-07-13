use crate::session_runtime::{SessionHandle, SessionMcpConnectionChange};

#[derive(Debug, thiserror::Error)]
pub enum SessionMcpError {
    #[error("{operation} requires a live targeted session")]
    LiveSessionRequired { operation: &'static str },
    #[error("MCP manager not enabled on this server")]
    ManagerNotEnabled,
    #[error("MCP enable requires an elicitation bridge")]
    ElicitationRequired,
}

fn require_runtime(
    runtime: Option<SessionHandle>,
    operation: &'static str,
) -> Result<SessionHandle, SessionMcpError> {
    runtime.ok_or(SessionMcpError::LiveSessionRequired { operation })
}

pub async fn status(
    runtime: Option<SessionHandle>,
) -> Result<coco_types::McpStatusResult, SessionMcpError> {
    let runtime = require_runtime(runtime, "mcp/status")?;
    Ok(runtime
        .mcp_status_result()
        .await
        .unwrap_or(coco_types::McpStatusResult {
            mcp_servers: Vec::new(),
        }))
}

pub async fn set_dynamic_servers(
    runtime: Option<SessionHandle>,
    servers: Vec<(String, coco_mcp::McpServerConfig)>,
) -> Result<Vec<String>, SessionMcpError> {
    let runtime = require_runtime(runtime, "mcp/setServers")?;
    runtime
        .set_dynamic_mcp_servers(servers)
        .await
        .ok_or(SessionMcpError::ManagerNotEnabled)
}

pub async fn reconnect(
    runtime: Option<SessionHandle>,
    server_name: &str,
    send_elicitation: coco_mcp::SendElicitation,
) -> Result<SessionMcpConnectionChange, SessionMcpError> {
    let runtime = require_runtime(runtime, "mcp/reconnect")?;
    runtime
        .reconnect_mcp_server(server_name, send_elicitation)
        .await
        .ok_or(SessionMcpError::ManagerNotEnabled)
}

pub async fn set_enabled(
    runtime: Option<SessionHandle>,
    server_name: &str,
    enabled: bool,
    send_elicitation: Option<coco_mcp::SendElicitation>,
) -> Result<SessionMcpConnectionChange, SessionMcpError> {
    let runtime = require_runtime(runtime, "mcp/toggle")?;
    if enabled && send_elicitation.is_none() {
        return Err(SessionMcpError::ElicitationRequired);
    }
    runtime
        .set_mcp_server_enabled(server_name, enabled, send_elicitation)
        .await
        .ok_or(SessionMcpError::ManagerNotEnabled)
}

pub async fn wrap_send_elicitation_with_hooks(
    runtime: &SessionHandle,
    server_name: String,
    base: coco_mcp::SendElicitation,
) -> coco_mcp::SendElicitation {
    runtime
        .wrap_send_elicitation_with_hooks(server_name, base)
        .await
}
