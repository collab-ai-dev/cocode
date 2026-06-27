use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Options for creating an MCP provider tool.
#[derive(Default)]
pub struct McpToolOptions {
    /// Label identifying the MCP server (required).
    pub server_label: String,
    /// Optional URL of the MCP server.
    pub server_url: Option<String>,
    /// Optional allowed tools (string array or `{ readOnly?, toolNames? }`).
    pub allowed_tools: Option<serde_json::Value>,
    /// Optional authorization header value.
    pub authorization: Option<String>,
    /// Optional connector identifier.
    pub connector_id: Option<String>,
    /// Optional HTTP headers for the MCP connection.
    pub headers: Option<HashMap<String, String>>,
    /// Optional approval mode ("always", "never", or `{ never: { toolNames? } }`).
    pub require_approval: Option<serde_json::Value>,
    /// Optional description of the server.
    pub server_description: Option<String>,
}

/// Create an mcp provider tool for the Responses API.
pub fn openai_mcp_tool(options: McpToolOptions) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    args.insert("server_label".into(), json!(options.server_label));
    if let Some(url) = options.server_url {
        args.insert("server_url".into(), json!(url));
    }
    if let Some(tools) = options.allowed_tools {
        args.insert("allowed_tools".into(), tools);
    }
    if let Some(auth) = options.authorization {
        args.insert("authorization".into(), json!(auth));
    }
    if let Some(cid) = options.connector_id {
        args.insert("connector_id".into(), json!(cid));
    }
    if let Some(hdrs) = options.headers {
        args.insert("headers".into(), json!(hdrs));
    }
    if let Some(approval) = options.require_approval {
        args.insert("require_approval".into(), approval);
    }
    if let Some(desc) = options.server_description {
        args.insert("server_description".into(), json!(desc));
    }
    LanguageModelV4ProviderTool {
        id: "openai.mcp".into(),
        name: "mcp".into(),
        args,
    }
}
