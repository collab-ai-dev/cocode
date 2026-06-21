//! MCP tool and resource discovery.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::info;

use crate::client::McpClientError;
use crate::client::McpConnectionManager;
use crate::naming::mcp_tool_id;
use crate::tool_call::truncate_description;
use crate::types::McpConnectionState;
use crate::types::McpToolDefinition;

/// Tool annotations from an MCP server.
#[derive(Debug, Clone, Default)]
pub struct ToolAnnotations {
    /// Whether the tool only reads data (safe for concurrent execution).
    pub read_only: bool,
    /// Whether the tool can destroy data.
    pub destructive: bool,
    /// Whether the tool accesses external resources.
    pub open_world: bool,
    /// Display title override.
    pub title: Option<String>,
}

/// A discovered tool with its server context.
#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    /// Fully qualified name: mcp__<server>__<tool>.
    pub fq_name: String,
    /// Server name.
    pub server_name: String,
    /// Original tool name on the server.
    pub tool_name: String,
    /// Tool description (truncated to limit).
    pub description: String,
    /// JSON Schema for tool input.
    pub input_schema: serde_json::Value,
    /// Tool annotations from the server.
    pub annotations: ToolAnnotations,
    /// Search hint for tool discovery.
    pub search_hint: Option<String>,
    /// Whether this tool should always be loaded (not deferred).
    pub always_load: bool,
}

/// A discovered resource with its server context.
#[derive(Debug, Clone)]
pub struct DiscoveredResource {
    pub server_name: String,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

/// Cache for discovered tools/resources per server.
#[derive(Debug, Default)]
pub struct DiscoveryCache {
    tools: HashMap<String, Vec<DiscoveredTool>>,
    resources: HashMap<String, Vec<DiscoveredResource>>,
}

impl DiscoveryCache {
    /// Get cached tools for a server.
    pub fn get_tools(&self, server_name: &str) -> Option<&Vec<DiscoveredTool>> {
        self.tools.get(server_name)
    }

    /// Get cached resources for a server.
    pub fn get_resources(&self, server_name: &str) -> Option<&Vec<DiscoveredResource>> {
        self.resources.get(server_name)
    }

    /// Store tools for a server.
    pub fn set_tools(&mut self, server_name: &str, tools: Vec<DiscoveredTool>) {
        self.tools.insert(server_name.to_string(), tools);
    }

    /// Store resources for a server.
    pub fn set_resources(&mut self, server_name: &str, resources: Vec<DiscoveredResource>) {
        self.resources.insert(server_name.to_string(), resources);
    }

    /// Clear cache for a specific server (e.g., after reconnect).
    pub fn invalidate(&mut self, server_name: &str) {
        self.tools.remove(server_name);
        self.resources.remove(server_name);
    }

    /// Clear all caches.
    pub fn clear(&mut self) {
        self.tools.clear();
        self.resources.clear();
    }
}

/// Discover tools from a connected MCP server.
///
/// Sends `tools/list`, converts to the internal format, truncates descriptions,
/// and extracts annotations.
pub async fn discover_tools_from_server(
    manager: &McpConnectionManager,
    server_name: &str,
    cache: &Arc<RwLock<DiscoveryCache>>,
) -> Result<Vec<DiscoveredTool>, McpClientError> {
    // Check cache first
    {
        let cache_guard = cache.read().await;
        if let Some(cached) = cache_guard.get_tools(server_name) {
            return Ok(cached.clone());
        }
    }

    // Get connected server
    let state = manager.get_state(server_name).await;
    let server = match state {
        Some(McpConnectionState::Connected(server)) => server,
        Some(McpConnectionState::NeedsAuth { auth_url }) => {
            return Err(McpClientError::AuthRequired { auth_url });
        }
        _ => {
            return Err(McpClientError::ServerNotFound {
                name: server_name.to_string(),
            });
        }
    };

    // Check if server supports tools capability
    if !server.capabilities.tools {
        info!(server = %server_name, "server does not support tools capability");
        return Ok(Vec::new());
    }

    // Convert server tools to discovered tools
    let tools = convert_server_tools(server_name, &server.tools);

    info!(
        server = %server_name,
        count = tools.len(),
        "discovered tools from MCP server"
    );

    // Update cache
    {
        let mut cache_guard = cache.write().await;
        cache_guard.set_tools(server_name, tools.clone());
    }

    Ok(tools)
}

/// Discover resources from a connected MCP server.
///
/// Sends `resources/list`.
pub async fn discover_resources(
    manager: &McpConnectionManager,
    server_name: &str,
    cache: &Arc<RwLock<DiscoveryCache>>,
) -> Result<Vec<DiscoveredResource>, McpClientError> {
    // Check cache first
    {
        let cache_guard = cache.read().await;
        if let Some(cached) = cache_guard.get_resources(server_name) {
            return Ok(cached.clone());
        }
    }

    // Get connected server
    let state = manager.get_state(server_name).await;
    let server = match state {
        Some(McpConnectionState::Connected(server)) => server,
        _ => {
            return Err(McpClientError::ServerNotFound {
                name: server_name.to_string(),
            });
        }
    };

    // Check if server supports resources capability
    if !server.capabilities.resources {
        info!(server = %server_name, "server does not support resources capability");
        return Ok(Vec::new());
    }

    // Convert server resources
    let resources: Vec<DiscoveredResource> = server
        .resources
        .iter()
        .map(|r| DiscoveredResource {
            server_name: server_name.to_string(),
            uri: r.uri.clone(),
            name: r.name.clone(),
            description: r.description.clone(),
            mime_type: r.mime_type.clone(),
        })
        .collect();

    info!(
        server = %server_name,
        count = resources.len(),
        "discovered resources from MCP server"
    );

    // Update cache
    {
        let mut cache_guard = cache.write().await;
        cache_guard.set_resources(server_name, resources.clone());
    }

    Ok(resources)
}

/// Refresh capabilities from a server after reconnect.
///
/// Invalidates the discovery cache for the server and re-fetches
/// tools and resources.
pub async fn refresh_server_capabilities(
    manager: &McpConnectionManager,
    server_name: &str,
    cache: &Arc<RwLock<DiscoveryCache>>,
) -> Result<ServerCapabilities, McpClientError> {
    // Invalidate cache
    {
        let mut cache_guard = cache.write().await;
        cache_guard.invalidate(server_name);
    }

    info!(server = %server_name, "refreshing server capabilities");

    // Re-discover tools
    let tools = discover_tools_from_server(manager, server_name, cache).await?;

    // Re-discover resources
    let resources = discover_resources(manager, server_name, cache).await?;

    Ok(ServerCapabilities { tools, resources })
}

/// Discover tools and resources from all connected servers.
pub async fn discover_all(
    manager: &McpConnectionManager,
    cache: &Arc<RwLock<DiscoveryCache>>,
) -> Vec<(String, Result<ServerCapabilities, McpClientError>)> {
    let servers = manager.connected_servers().await;
    let mut results = Vec::new();

    for server in &servers {
        let tools = discover_tools_from_server(manager, &server.name, cache).await;
        let resources = discover_resources(manager, &server.name, cache).await;

        let result = match (tools, resources) {
            (Ok(tools), Ok(resources)) => Ok(ServerCapabilities { tools, resources }),
            (Err(e), _) | (_, Err(e)) => Err(e),
        };

        results.push((server.name.clone(), result));
    }

    results
}

/// Combined tools and resources for a server.
#[derive(Debug, Clone)]
pub struct ServerCapabilities {
    pub tools: Vec<DiscoveredTool>,
    pub resources: Vec<DiscoveredResource>,
}

// ── Dynamic resource discovery ──

/// Query parameters for runtime resource discovery by URI pattern,
/// name prefix, or MIME type.
#[derive(Debug, Clone, Default)]
pub struct DynamicResourceQuery {
    /// URI prefix filter (e.g. `"file://"`, `"https://api.example.com/"`).
    pub uri_prefix: Option<String>,
    /// Name substring filter (case-insensitive).
    pub name_contains: Option<String>,
    /// MIME type filter (exact match, e.g. `"application/json"`).
    pub mime_type: Option<String>,
}

impl DynamicResourceQuery {
    /// Whether a discovered resource matches this query.
    pub fn matches(&self, resource: &DiscoveredResource) -> bool {
        if let Some(prefix) = &self.uri_prefix
            && !resource.uri.starts_with(prefix.as_str())
        {
            return false;
        }
        if let Some(name_filter) = &self.name_contains
            && !resource
                .name
                .to_lowercase()
                .contains(&name_filter.to_lowercase())
        {
            return false;
        }
        if let Some(mime) = &self.mime_type {
            match &resource.mime_type {
                Some(actual) if actual == mime => {}
                _ => return false,
            }
        }
        true
    }
}

/// Discover resources matching a query across all connected servers.
///
/// Queries each server's cached resources and applies the filter locally.
pub async fn discover_resources_matching(
    manager: &McpConnectionManager,
    cache: &Arc<RwLock<DiscoveryCache>>,
    query: &DynamicResourceQuery,
) -> Vec<DiscoveredResource> {
    let servers = manager.connected_servers().await;
    let mut results = Vec::new();

    for server in &servers {
        let resources = discover_resources(manager, &server.name, cache).await;
        if let Ok(resources) = resources {
            results.extend(resources.into_iter().filter(|r| query.matches(r)));
        }
    }

    results
}

/// Convert raw MCP tool definitions to discovered tools.
///
/// Applies description truncation and extracts annotations from
/// the tool metadata.
fn convert_server_tools(server_name: &str, tools: &[McpToolDefinition]) -> Vec<DiscoveredTool> {
    tools
        .iter()
        .map(|tool| {
            let description = tool
                .description
                .as_deref()
                .map(truncate_description)
                .unwrap_or_default();

            // Extract annotations from input schema metadata
            let annotations = extract_annotations(&tool.input_schema);

            // Extract search hint from metadata. Anthropic-namespaced key
            // first (claude-code MCP compat), then a provider-neutral
            // `searchHint` fallback so non-Anthropic servers can supply one.
            let search_hint = tool
                .input_schema
                .get("_meta")
                .and_then(|m| {
                    m.get("anthropic/searchHint")
                        .or_else(|| m.get("searchHint"))
                })
                .and_then(|h| h.as_str())
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "));

            // Anthropic-namespaced key first (claude-code MCP compat), then a
            // provider-neutral `alwaysLoad` fallback.
            let always_load = tool
                .input_schema
                .get("_meta")
                .and_then(|m| {
                    m.get("anthropic/alwaysLoad")
                        .or_else(|| m.get("alwaysLoad"))
                })
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            DiscoveredTool {
                fq_name: mcp_tool_id(server_name, &tool.name),
                server_name: server_name.to_string(),
                tool_name: tool.name.clone(),
                description,
                input_schema: tool.input_schema.clone(),
                annotations,
                search_hint,
                always_load,
            }
        })
        .collect()
}

/// Extract tool annotations from the input schema.
fn extract_annotations(schema: &serde_json::Value) -> ToolAnnotations {
    let annotations = schema.get("annotations");

    ToolAnnotations {
        read_only: annotations
            .and_then(|a| a.get("readOnlyHint"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        destructive: annotations
            .and_then(|a| a.get("destructiveHint"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        open_world: annotations
            .and_then(|a| a.get("openWorldHint"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        title: annotations
            .and_then(|a| a.get("title"))
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

#[cfg(test)]
#[path = "discovery.test.rs"]
mod tests;
