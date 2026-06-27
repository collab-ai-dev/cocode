//! MCP server lifecycle, config, auth, transport channels.
//!
//! Uses `coco-rmcp-client` for actual MCP protocol communication via the `rmcp`
//! SDK. This crate adds coco-specific business logic: naming normalization,
//! config scopes, discovery caching, file watching.

pub mod auth;
pub mod auth_cache;
pub mod channel_permission;
pub mod client;
pub mod config;
pub mod config_watcher;
pub mod discovery;
pub mod elicitation;
mod env_expansion;
pub mod naming;
pub mod tool_call;
pub mod types;
pub mod xaa;
pub mod xaa_idp_login;

pub use auth::OAuthConfig;
pub use auth::OAuthTokenStore;
pub use auth::OAuthTokens;
pub use channel_permission::ChannelPermission;
pub use channel_permission::ChannelPermissionRelay;
pub use channel_permission::DenyAllRelay;
pub use channel_permission::StaticPermissionRelay;
pub use client::McpClientError;
pub use client::McpConnectionManager;
pub use client::SdkRouteFuture;
pub use client::SdkRouteMessage;
pub use config::McpConfigLoader;
pub use config_watcher::McpConfigChanged;
pub use config_watcher::watch_mcp_configs;
pub use discovery::DiscoveredResource;
pub use discovery::DiscoveredTool;
pub use discovery::DiscoveryCache;
pub use discovery::DynamicResourceQuery;
pub use discovery::ServerCapabilities;
pub use discovery::ToolAnnotations;
pub use discovery::discover_all;
pub use discovery::discover_resources;
pub use discovery::discover_resources_matching;
pub use discovery::discover_tools_from_server;
pub use discovery::refresh_server_capabilities;
pub use elicitation::ElicitResult;
pub use elicitation::ElicitationField;
pub use elicitation::ElicitationFieldType;
pub use elicitation::ElicitationMode;
pub use elicitation::ElicitationRequest;
pub use elicitation::ElicitationResult;
pub use elicitation::ElicitationType;
pub use naming::mcp_tool_id;
pub use naming::parse_mcp_tool_id;
pub use naming::parse_permission_reply;
pub use naming::short_request_id;
pub use types::ConfigScope;
pub use types::ConnectedMcpServer;
pub use types::McpCapabilities;
pub use types::McpConnectionState;
pub use types::McpResource;
pub use types::McpServerConfig;
pub use types::McpToolDefinition;
pub use types::McpTransport;
pub use types::ScopedMcpServerConfig;

// Re-export rmcp client types for consumers that need direct access
pub use coco_mcp_types::BlobResourceContents;
pub use coco_mcp_types::ReadResourceResult;
pub use coco_mcp_types::ReadResourceResultContents;
pub use coco_mcp_types::TextResourceContents;
/// Re-export the rmcp wire enum so consumers (CLI / SDK) can build
/// elicitation responses without depending on `rmcp` directly.
pub use coco_rmcp_client::Elicitation;
pub use coco_rmcp_client::ElicitationAction as RmcpElicitationAction;
pub use coco_rmcp_client::ElicitationResponse;
pub use coco_rmcp_client::McpAuthStatus;
pub use coco_rmcp_client::RmcpClient;
pub use coco_rmcp_client::RmcpClientError;
pub use coco_rmcp_client::SendElicitation;
