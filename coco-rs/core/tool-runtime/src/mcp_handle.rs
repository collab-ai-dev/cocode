//! MCP handle trait — abstraction for MCP operations from tools.
//!
//! Same pattern as [`SideQuery`](crate::side_query): trait defined here,
//! implementation in the MCP service layer, injected via `ToolUseContext`.

use serde_json::Value;
use std::sync::Arc;

/// A resource from an MCP server.
#[derive(Debug, Clone)]
pub struct McpResourceInfo {
    pub server_name: String,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

/// One content item returned by an MCP resource read.
#[derive(Debug, Clone)]
pub struct McpResourceContent {
    pub uri: String,
    pub text: Option<String>,
    pub blob: Option<String>,
    pub mime_type: Option<String>,
}

/// Result from an MCP tool call.
#[derive(Debug, Clone)]
pub struct McpToolCallResult {
    pub content: Vec<McpContentBlock>,
    pub is_error: bool,
}

/// A content block in an MCP result.
#[derive(Debug, Clone)]
pub enum McpContentBlock {
    Text(String),
    Image {
        data: String,
        mime_type: String,
    },
    Audio {
        data: String,
        mime_type: String,
    },
    Resource {
        uri: String,
        text: Option<String>,
        blob: Option<String>,
        mime_type: Option<String>,
    },
}

/// MCP tool annotations — safety hints from the MCP server.
///
/// These are server-declared hints, not guarantees. Used for concurrency
/// batching and permission decisions.
#[derive(Debug, Clone, Default)]
pub struct McpToolAnnotations {
    /// Tool only reads data, no side effects. Default: false.
    pub read_only_hint: bool,
    /// Tool can destroy or irreversibly modify data. Default: false.
    pub destructive_hint: bool,
    /// Tool accesses external resources (network, APIs). Default: false.
    pub open_world_hint: bool,
    /// Server-side opt-out from `ToolSearch` deferral. When the MCP server
    /// advertises `_meta["anthropic/alwaysLoad"] == true` (or the
    /// provider-neutral `_meta["alwaysLoad"]`) on a tool, this flag
    /// short-circuits the deferred-pool filter in
    /// [`crate::ToolRegistry::loaded_tools`], so the tool's full
    /// schema appears in turn-1 tool definitions.
    ///
    /// Default: false (every MCP tool is deferred unless the server opts out).
    pub always_load: bool,
    /// Server-declared `ToolSearch` hint, lifted from the tool's `_meta`.
    /// Curated capability phrase that boosts this (deferred) MCP tool in
    /// `ToolSearch` ranking. `None` when the server advertises no hint.
    pub search_hint: Option<String>,
}

impl McpToolAnnotations {
    /// Read the MCP server-declared `_meta` block off the tool's input
    /// schema and lift the `alwaysLoad` flag + the search hint onto the
    /// typed annotation struct. Other annotation fields stay at
    /// [`Default::default()`] — they are wired by the discovery layer
    /// from the rmcp `annotations` object, not from `_meta`.
    ///
    pub fn from_input_schema_meta(input_schema: &Value) -> Self {
        let meta = input_schema.get("_meta");
        // Both `_meta` flags accept the Anthropic-namespaced key first (compat
        // with claude-code MCP servers), then a provider-neutral fallback so
        // non-Anthropic servers can supply them too.
        let always_load = meta
            .and_then(|m| {
                m.get("anthropic/alwaysLoad")
                    .or_else(|| m.get("alwaysLoad"))
            })
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // `searchHint` is whitespace-normalized to a single-spaced phrase;
        // empty strings collapse to `None`.
        let search_hint = meta
            .and_then(|m| {
                m.get("anthropic/searchHint")
                    .or_else(|| m.get("searchHint"))
            })
            .and_then(Value::as_str)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty());
        Self {
            always_load,
            search_hint,
            ..Self::default()
        }
    }
}

/// Schema info for an MCP server tool.
#[derive(Debug, Clone)]
pub struct McpToolSchema {
    pub server_name: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    /// Safety annotations from the MCP server.
    pub annotations: McpToolAnnotations,
}

/// Trait for MCP operations from tools.
///
/// Wraps `McpConnectionManager` — tools call this instead of
/// depending on `coco-mcp` directly.
#[async_trait::async_trait]
pub trait McpHandle: Send + Sync {
    /// List resources from an MCP server.
    async fn list_resources(
        &self,
        server_name: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError>;

    /// Read a resource from an MCP server.
    async fn read_resource(
        &self,
        server_name: &str,
        resource_uri: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError>;

    /// Call an MCP tool.
    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError>;

    /// Initiate authentication for an MCP server.
    async fn authenticate(&self, server_name: &str) -> Result<String, coco_error::BoxedError>;

    /// Get names of all connected servers.
    async fn connected_servers(&self) -> Vec<String>;

    /// Names of servers still in the connecting / handshake phase. Surfaced by
    /// `ToolSearchTool` in its empty-match branch so the model knows
    /// to retry once handshakes complete. Default impl returns empty —
    /// handles without server-state tracking just say "no pending".
    async fn pending_server_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// List all tools from all connected MCP servers.
    ///
    /// Used by MCPTool to discover and expose MCP server tools to the LLM.
    async fn list_tools(&self) -> Vec<McpToolSchema> {
        vec![]
    }

    /// Register and connect a dynamically-defined MCP server.
    /// Used by per-agent MCP initialization to stand up agent-private
    /// servers from inline frontmatter
    /// configs. Returns the server name on success so the caller can
    /// pair it with the matching `remove_dynamic_server` at agent
    /// teardown.
    ///
    /// `config` is the JSON body of `McpServerConfig` (transport,
    /// command, env, …). The default impl returns an error so
    /// handles without a real MCP layer surface a clean failure.
    async fn add_dynamic_server(
        &self,
        _name: &str,
        _config: Value,
    ) -> Result<(), coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "MCP add_dynamic_server not supported in this context",
            coco_error::StatusCode::Internal,
        )))
    }

    /// Disconnect + deregister a dynamically-added server. Mirror of
    /// `add_dynamic_server`. Called at SubagentStop for every server
    /// that was spun up via inline config so agent-private servers
    /// don't outlive their agent.
    async fn remove_dynamic_server(&self, _name: &str) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
}

pub type McpHandleRef = Arc<dyn McpHandle>;

/// No-op implementation for contexts without MCP (tests, subagents).
#[derive(Debug, Clone)]
pub struct NoOpMcpHandle;

#[async_trait::async_trait]
impl McpHandle for NoOpMcpHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }
    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "MCP not available in this context",
            coco_error::StatusCode::Internal,
        )))
    }
    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "MCP not available in this context",
            coco_error::StatusCode::Internal,
        )))
    }
    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "MCP not available in this context",
            coco_error::StatusCode::Internal,
        )))
    }
    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}

#[cfg(test)]
#[path = "mcp_handle.test.rs"]
mod tests;
