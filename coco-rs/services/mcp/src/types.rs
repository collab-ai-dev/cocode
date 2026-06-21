//! MCP types — configuration, transport, connection state.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// MCP configuration scope (source priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    Local,
    User,
    Project,
    Dynamic,
    Enterprise,
    ClaudeAi,
    Managed,
}

/// MCP transport type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Sse,
    SseIde,
    Http,
    WebSocket,
    Sdk,
    ClaudeAiProxy,
}

/// MCP server configuration — discriminated by transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerConfig {
    Stdio(McpStdioConfig),
    Sse(McpSseConfig),
    Http(McpHttpConfig),
    WebSocket(McpWsConfig),
    Sdk(McpSdkConfig),
    ClaudeAiProxy(McpClaudeAiProxyConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStdioConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSseConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(
        default,
        alias = "headersHelper",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers_helper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpHttpConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(
        default,
        alias = "headersHelper",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers_helper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xaa: Option<McpXaaConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpXaaConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idp_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idp_client_secret: Option<String>,
    #[serde(default, alias = "idToken", skip_serializing_if = "Option::is_none")]
    pub idp_id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idp_token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpWsConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(
        default,
        alias = "headersHelper",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers_helper: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSdkConfig {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClaudeAiProxyConfig {
    pub url: String,
    pub server_id: String,
}

/// A scoped config entry with source tracking.
#[derive(Debug, Clone)]
pub struct ScopedMcpServerConfig {
    pub name: String,
    pub config: McpServerConfig,
    pub scope: ConfigScope,
    pub plugin_source: Option<String>,
}

/// MCP connection state machine.
#[derive(Debug, Clone)]
pub enum McpConnectionState {
    Connected(ConnectedMcpServer),
    Failed { error: String },
    NeedsAuth { auth_url: Option<String> },
    Pending { reconnect_attempts: i32 },
    Disabled,
}

/// MCP server capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub channel: bool,
    pub channel_permission: bool,
}

/// A connected MCP server with its capabilities and tools.
#[derive(Debug, Clone)]
pub struct ConnectedMcpServer {
    pub name: String,
    pub capabilities: McpCapabilities,
    pub instructions: Option<String>,
    pub tools: Vec<McpToolDefinition>,
    pub resources: Vec<McpResource>,
    pub commands: Vec<McpPrompt>,
}

/// A tool provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// A resource provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A prompt/command provided by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
