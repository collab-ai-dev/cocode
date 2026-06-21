use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::McpToolInfo;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::tool_result_storage;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Max chars of an MCP server-supplied tool description surfaced to the model.
const MAX_MCP_DESCRIPTION_LENGTH: usize = 2048;

const MCP_AUTH_PROMPT: &str = "Authenticate with an MCP server by name to enable its tools and resources. Prefer a server's own `mcp__<server>__authenticate` tool when one is offered — use this generic tool only as a fallback for a server that needs authentication but is not already surfacing its own authenticate tool. Call with the server name to start the OAuth flow — you'll receive an authorization URL to share with the user; once the user authorizes in their browser, the server's real tools become available automatically.";

const LIST_MCP_RESOURCES_DESCRIPTION: &str = "Lists available resources from configured MCP servers.\nEach resource object includes a 'server' field indicating which server it's from.\n\nUsage examples:\n- List all resources from all servers: `listMcpResources`\n- List resources from a specific server: `listMcpResources({ server: \"myserver\" })`";

const LIST_MCP_RESOURCES_PROMPT: &str = "List available resources from configured MCP servers.\nEach returned resource will include all standard MCP resource fields plus a 'server' field\nindicating which server the resource belongs to.\n\nParameters:\n- server (optional): The name of a specific MCP server to get resources from. If not provided,\n  resources from all servers will be returned.";

const READ_MCP_RESOURCE_DESCRIPTION: &str = "Reads a specific resource from an MCP server.\n- server: The name of the MCP server to read from\n- uri: The URI of the resource to read\n\nUsage examples:\n- Read a resource from a server: `readMcpResource({ server: \"myserver\", uri: \"my-resource-uri\" })`";

const READ_MCP_RESOURCE_PROMPT: &str = "Reads a specific resource from an MCP server, identified by server name and resource URI.\n\nParameters:\n- server (required): The name of the MCP server from which to read the resource\n- uri (required): The URI of the resource to read";

/// Typed input for [`McpAuthTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct McpAuthInput {
    /// Name of the MCP server to authenticate with
    #[serde(default)]
    pub server_name: String,
}

pub struct McpAuthTool;

#[async_trait::async_trait]
impl Tool for McpAuthTool {
    type Input = McpAuthInput;
    coco_tool_runtime::impl_runtime_schema!(McpAuthInput);
    /// Output is the bare status string from the MCP authenticator —
    /// rendered unwrapped so the model sees readable prose, not a
    /// JSON-quoted string.
    type Output = String;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::McpAuth)
    }
    fn name(&self) -> &str {
        ToolName::McpAuth.as_str()
    }
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Chars(10_000)
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Mcp)
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn description(&self, _input: &McpAuthInput, _options: &DescriptionOptions) -> String {
        "Authenticate with an MCP server by name (fallback when a server isn't surfacing its own authenticate tool).".into()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        MCP_AUTH_PROMPT.into()
    }

    async fn check_permissions(
        &self,
        input: &McpAuthInput,
        _ctx: &ToolUseContext,
    ) -> coco_types::ToolCheckResult {
        coco_types::ToolCheckResult::Allow {
            updated_input: serde_json::to_value(input).ok(),
            feedback: None,
        }
    }

    fn to_auto_classifier_input(&self, input: &McpAuthInput) -> Option<String> {
        Some(input.server_name.clone())
    }

    fn render_for_model(&self, out: &String) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: McpAuthInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<String>, ToolError> {
        if input.server_name.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "server_name is required".into(),
                error_code: None,
            });
        }

        let message = match ctx.mcp.authenticate(&input.server_name).await {
            Ok(msg) => msg,
            Err(e) => format!(
                "Authentication failed for {server}: {e}",
                server = input.server_name
            ),
        };

        Ok(ToolResult {
            data: message,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Empty input for [`McpAuthServerTool`] — the server is baked into the tool,
/// so the call takes no arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct McpAuthServerInput {}

/// Per-server `mcp__<server>__authenticate` pseudo-tool, surfaced in place of
/// a `NeedsAuth` server's real tools so the model is told *which* server needs
/// authentication and can start the OAuth flow on the user's behalf.
///
/// Unlike the global
/// [`McpAuthTool`] (free-form `server_name` input), this pre-binds the server so
/// the model can't guess the wrong name, and it self-removes on a successful
/// reconnect: it reports `mcp_info().server_name == server`, so the
/// `ToolRegistry::replace_server_tools` wipe that installs the real tools
/// removes the pseudo-tool in the same atomic swap as the
/// `mcp__<server>__*` prefix replacement). It defers like other MCP tools;
/// pending-server state keeps `ToolSearch` visible while auth/bootstrap is
/// incomplete.
pub struct McpAuthServerTool {
    info: McpToolInfo,
    qualified_name: String,
    description: String,
}

impl McpAuthServerTool {
    /// `transport` is the wire transport label (e.g. `"http"` / `"sse"`); `url`
    /// is the server endpoint when the transport has one. Both feed the
    /// model-facing description so it knows exactly what it is authenticating.
    pub fn new(server_name: String, transport: &str, url: Option<&str>) -> Self {
        let location = match url {
            Some(url) => format!("{transport} at {url}"),
            None => transport.to_string(),
        };
        let description = format!(
            "The `{server_name}` MCP server ({location}) is installed but requires \
             authentication. Call this tool to start the OAuth flow — you'll receive an \
             authorization URL to share with the user. Once the user completes authorization \
             in their browser, the server's real tools become available automatically."
        );
        let info = McpToolInfo {
            server_name,
            tool_name: "authenticate".to_string(),
        };
        let qualified_name = info.qualified_name();
        Self {
            info,
            qualified_name,
            description,
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpAuthServerTool {
    type Input = McpAuthServerInput;
    coco_tool_runtime::impl_runtime_schema!(McpAuthServerInput);
    type Output = String;

    fn id(&self) -> ToolId {
        ToolId::Mcp {
            server: self.info.server_name.clone(),
            tool: self.info.tool_name.clone(),
        }
    }
    fn name(&self) -> &str {
        &self.qualified_name
    }
    fn mcp_info(&self) -> Option<&McpToolInfo> {
        Some(&self.info)
    }
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Chars(10_000)
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Mcp)
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn description(&self, _input: &McpAuthServerInput, _options: &DescriptionOptions) -> String {
        self.description.clone()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        self.description.clone()
    }

    async fn check_permissions(
        &self,
        input: &McpAuthServerInput,
        _ctx: &ToolUseContext,
    ) -> coco_types::ToolCheckResult {
        coco_types::ToolCheckResult::Allow {
            updated_input: serde_json::to_value(input).ok(),
            feedback: None,
        }
    }

    fn to_auto_classifier_input(&self, _input: &McpAuthServerInput) -> Option<String> {
        Some(self.info.server_name.clone())
    }

    fn render_for_model(&self, out: &String) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        _input: McpAuthServerInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<String>, ToolError> {
        let server = &self.info.server_name;
        let message = match ctx.mcp.authenticate(server).await {
            Ok(msg) => msg,
            Err(e) => format!("Authentication failed for {server}: {e}"),
        };
        Ok(ToolResult {
            data: message,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Typed input for [`ListMcpResourcesTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListMcpResourcesInput {
    /// Optional server name to filter resources by
    #[serde(default, rename = "server")]
    pub server_name: Option<String>,
}

pub struct ListMcpResourcesTool;

#[async_trait::async_trait]
impl Tool for ListMcpResourcesTool {
    type Input = ListMcpResourcesInput;
    coco_tool_runtime::impl_runtime_schema!(ListMcpResourcesInput);
    /// Output is `Value` because the wire shape is a union (bare
    /// status string for empty/error, JSON array for results).
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ListMcpResources)
    }
    fn name(&self) -> &str {
        ToolName::ListMcpResources.as_str()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Mcp)
    }
    fn description(&self, _input: &ListMcpResourcesInput, _options: &DescriptionOptions) -> String {
        LIST_MCP_RESOURCES_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        LIST_MCP_RESOURCES_PROMPT.into()
    }
    fn is_read_only(&self, _input: &ListMcpResourcesInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    /// Listing resources from one or more MCP servers is read-only and
    /// independent across servers — the executor can fan out concurrent
    /// listing calls.
    fn is_concurrency_safe(&self, _input: &ListMcpResourcesInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("list resources from connected MCP servers")
    }

    /// Unwraps the bare string for empty/error branches and JSON-stringifies
    /// the array for non-empty results.
    fn render_for_model(&self, out: &Value) -> Vec<ToolResultContentPart> {
        coco_tool_runtime::render_text_or_json(out)
    }

    async fn execute(
        &self,
        input: ListMcpResourcesInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let server_name = input.server_name.as_deref();

        match ctx.mcp.list_resources(server_name).await {
            Ok(resources) => {
                if resources.is_empty() {
                    return Ok(ToolResult {
                        data: serde_json::json!(
                            "No resources found. MCP servers may still provide tools even if they have no resources."
                        ),
                        new_messages: vec![],
                        app_state_patch: None,
                        permission_updates: Vec::new(),
                        display_data: None,
                    });
                }
                let items: Vec<Value> = resources
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "server_name": r.server_name,
                            "uri": r.uri,
                            "name": r.name,
                            "description": r.description,
                            "mime_type": r.mime_type,
                        })
                    })
                    .collect();
                Ok(ToolResult {
                    data: serde_json::json!(items),
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                })
            }
            Err(e) => Ok(ToolResult {
                data: serde_json::json!(format!("Failed to list resources: {e}")),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            }),
        }
    }
}

/// Typed input for [`ReadMcpResourceTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadMcpResourceInput {
    /// The MCP server name
    #[serde(rename = "server")]
    pub server_name: String,
    /// The resource URI to read
    #[serde(rename = "uri")]
    pub resource_uri: String,
}

pub struct ReadMcpResourceTool;

#[async_trait::async_trait]
impl Tool for ReadMcpResourceTool {
    type Input = ReadMcpResourceInput;
    coco_tool_runtime::impl_runtime_schema!(ReadMcpResourceInput);
    /// Output is `Value` because the wire shape varies: single content
    /// envelope, multi-content `{contents: [...]}`, or a bare error
    /// string. The renderer treats them uniformly.
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ReadMcpResource)
    }
    fn name(&self) -> &str {
        ToolName::ReadMcpResource.as_str()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Mcp)
    }
    fn description(&self, _input: &ReadMcpResourceInput, _options: &DescriptionOptions) -> String {
        READ_MCP_RESOURCE_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        READ_MCP_RESOURCE_PROMPT.into()
    }
    fn is_read_only(&self, _input: &ReadMcpResourceInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    /// Resource reads are side-effect-free; multiple reads to the same or
    /// different resources can run in parallel.
    fn is_concurrency_safe(&self, _input: &ReadMcpResourceInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("read a specific MCP resource by URI")
    }

    /// Unwraps the error-path bare string (which would otherwise be
    /// JSON-quoted) so errors render as plain text.
    fn render_for_model(&self, out: &Value) -> Vec<ToolResultContentPart> {
        if let Some(text) = out.get("persisted_output").and_then(Value::as_str) {
            return vec![ToolResultContentPart::Text {
                text: text.to_string(),
                provider_options: None,
            }];
        }
        coco_tool_runtime::render_text_or_json(out)
    }

    async fn execute(
        &self,
        input: ReadMcpResourceInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        if input.server_name.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "server_name is required".into(),
                error_code: None,
            });
        }
        if input.resource_uri.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "resource_uri is required".into(),
                error_code: None,
            });
        }

        match ctx
            .mcp
            .read_resource(&input.server_name, &input.resource_uri)
            .await
        {
            Ok(contents) => {
                let total = contents.len();
                let mut rendered = Vec::with_capacity(total);
                for (idx, content) in contents.iter().enumerate() {
                    rendered
                        .push(read_mcp_resource_content_for_model(ctx, content, idx, total).await);
                }
                let data = if rendered.len() == 1 {
                    rendered.remove(0)
                } else {
                    serde_json::json!({ "contents": rendered })
                };
                Ok(ToolResult {
                    data,
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                })
            }
            Err(e) => Ok(ToolResult {
                data: serde_json::json!(format!(
                    "Failed to read resource {uri} from {server}: {e}",
                    uri = input.resource_uri,
                    server = input.server_name
                )),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            }),
        }
    }
}

/// Dynamic MCP tool wrapper — exposes MCP server tools to the LLM.
///
/// Each MCPTool instance wraps one specific MCP server tool. The registry
/// creates one MCPTool per discovered MCP server tool at startup and when
/// MCP servers connect/disconnect.
pub struct McpTool {
    info: McpToolInfo,
    tool_description: String,
    /// Self-validating schema (v4.2) built from the wire schema at
    /// construction — owns both the document (model-facing, via
    /// [`coco_tool_runtime::ToolInputSchema::as_value`]) and the compiled
    /// validator (runtime). Replaces the old `raw_schema` map; `from_value`
    /// folds in `"type":"object"` for servers that omit it.
    schema: coco_tool_runtime::ToolInputSchema,
    annotations: coco_tool_runtime::McpToolAnnotations,
}

impl McpTool {
    pub fn new(
        server_name: String,
        tool_name: String,
        description: String,
        schema: Value,
        annotations: coco_tool_runtime::McpToolAnnotations,
    ) -> Result<Self, coco_tool_runtime::SchemaError> {
        // Non-object / absent payload → canonical empty-params envelope.
        // `from_value` folds in `"type":"object"` when the server omits it
        // and compiles the validator (= meta-validation); an uncompilable
        // wire schema surfaces as `Err` and the tool is skipped at registration.
        let raw = match schema {
            Value::Object(_) => schema,
            _ => serde_json::json!({ "properties": {} }),
        };
        let schema = coco_tool_runtime::ToolInputSchema::from_value(raw)?;
        Ok(Self {
            info: McpToolInfo {
                server_name,
                tool_name,
            },
            tool_description: description,
            schema,
            annotations,
        })
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    /// `McpTool` is the **dynamic** wrapper — its input schema is
    /// supplied by the connected MCP server at runtime via
    /// `self.info.input_schema`. No compile-time Rust type can describe
    /// it, so `Value` is the correct assoc type here (and only here).
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        &self.schema
    }

    fn id(&self) -> ToolId {
        ToolId::Mcp {
            server: self.info.server_name.clone(),
            tool: self.info.tool_name.clone(),
        }
    }

    fn name(&self) -> &str {
        &self.info.tool_name
    }

    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Mcp)
    }

    fn description(&self, _: &Value, _options: &DescriptionOptions) -> String {
        self.tool_description.clone()
    }

    /// Model-facing description = the server-supplied tool description,
    /// truncated to [`MAX_MCP_DESCRIPTION_LENGTH`].
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        if self.tool_description.chars().count() > MAX_MCP_DESCRIPTION_LENGTH {
            let truncated: String = self
                .tool_description
                .chars()
                .take(MAX_MCP_DESCRIPTION_LENGTH)
                .collect();
            format!("{truncated}… [truncated]")
        } else {
            self.tool_description.clone()
        }
    }

    fn mcp_info(&self) -> Option<&McpToolInfo> {
        Some(&self.info)
    }

    /// Defers every MCP tool by default. The model must call `ToolSearch` to
    /// bring an MCP tool's full schema into the request — unless the server
    /// advertised `_meta["anthropic/alwaysLoad"] == true`, which routes through
    /// [`Self::always_load`] and short-circuits the deferred-pool filter in
    /// `ToolRegistry::loaded_tools`.
    fn should_defer(&self) -> bool {
        true
    }

    /// Read from `McpToolAnnotations.always_load`, sourced from the server's
    /// `_meta["anthropic/alwaysLoad"]` (or provider-neutral `_meta["alwaysLoad"]`)
    /// flag on the tool. When true, `ToolRegistry::loaded_tools` ignores the
    /// `should_defer()` signal and surfaces the tool's full schema on turn 1.
    fn always_load(&self) -> bool {
        self.annotations.always_load
    }

    /// Server-declared search hint, lifted from the tool's
    /// `_meta["anthropic/searchHint"]` (or the provider-neutral
    /// `_meta["searchHint"]`) by [`McpToolAnnotations::from_input_schema_meta`].
    /// Feeds `ToolSearch` ranking for this deferred MCP tool.
    fn search_hint(&self) -> Option<&str> {
        self.annotations.search_hint.as_deref()
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // Only concurrent-safe if the server declares read-only.
        self.annotations.read_only_hint
    }

    fn is_read_only(&self, _: &Value) -> bool {
        self.annotations.read_only_hint
    }

    fn is_destructive(&self, _: &Value) -> bool {
        self.annotations.destructive_hint
    }

    /// Decode the MCP server-provided content envelope back into typed
    /// `ToolResultContentPart`s. The `execute` path serializes
    /// `result.content` into a JSON array of `{type, ...}` blocks
    /// (success: bare array; error: `{error, content: [...]}`).
    /// `render_for_model` reverses that step so multimodal-capable
    /// providers see the original Text + FileData (image) parts the
    /// server emitted, instead of an opaque JSON-stringified envelope.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let arr = data
            .as_array()
            .or_else(|| data.get("content").and_then(Value::as_array));
        let Some(blocks) = arr else {
            return vec![ToolResultContentPart::Text {
                text: serde_json::to_string(data).unwrap_or_default(),
                provider_options: None,
            }];
        };
        let parts: Vec<ToolResultContentPart> = blocks
            .iter()
            .filter_map(|block| {
                let kind = block.get("type")?.as_str()?;
                match kind {
                    "text" => Some(ToolResultContentPart::Text {
                        text: block.get("text")?.as_str()?.to_string(),
                        provider_options: None,
                    }),
                    "image" => Some(ToolResultContentPart::FileData {
                        data: block.get("data")?.as_str()?.to_string(),
                        media_type: block
                            .get("mime_type")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png")
                            .to_string(),
                        filename: None,
                        provider_options: None,
                    }),
                    "resource" => block.get("text").and_then(Value::as_str).map(|text| {
                        ToolResultContentPart::Text {
                            text: text.to_string(),
                            provider_options: None,
                        }
                    }),
                    _ => None,
                }
            })
            .collect();
        if parts.is_empty() {
            return vec![ToolResultContentPart::Text {
                text: serde_json::to_string(data).unwrap_or_default(),
                provider_options: None,
            }];
        }
        parts
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let arguments =
            if input.is_null() || input.as_object().is_some_and(serde_json::Map::is_empty) {
                None
            } else {
                Some(input)
            };

        match ctx
            .mcp
            .call_tool(&self.info.server_name, &self.info.tool_name, arguments)
            .await
        {
            Ok(result) => {
                let mut content: Vec<Value> = Vec::with_capacity(result.content.len());
                for (idx, block) in result.content.iter().enumerate() {
                    let value = match block {
                        coco_tool_runtime::mcp_handle::McpContentBlock::Text(text) => {
                            serde_json::json!({"type": "text", "text": text})
                        }
                        coco_tool_runtime::mcp_handle::McpContentBlock::Image {
                            data,
                            mime_type,
                        } => {
                            serde_json::json!({"type": "image", "data": data, "mime_type": mime_type})
                        }
                        coco_tool_runtime::mcp_handle::McpContentBlock::Audio {
                            data,
                            mime_type,
                        } => {
                            mcp_binary_block_for_model(
                                ctx,
                                &self.info.tool_name,
                                idx,
                                result.content.len(),
                                data,
                                Some(mime_type),
                            )
                            .await
                        }
                        coco_tool_runtime::mcp_handle::McpContentBlock::Resource {
                            uri,
                            text: Some(text),
                            mime_type,
                            ..
                        } => serde_json::json!({
                            "type": "resource",
                            "uri": uri,
                            "text": text,
                            "mime_type": mime_type,
                        }),
                        coco_tool_runtime::mcp_handle::McpContentBlock::Resource {
                            blob: Some(blob),
                            mime_type,
                            ..
                        } => {
                            mcp_binary_block_for_model(
                                ctx,
                                &self.info.tool_name,
                                idx,
                                result.content.len(),
                                blob,
                                mime_type.as_deref(),
                            )
                            .await
                        }
                        coco_tool_runtime::mcp_handle::McpContentBlock::Resource {
                            uri, ..
                        } => {
                            serde_json::json!({"type": "text", "text": format!("[Resource: {uri}]")})
                        }
                    };
                    content.push(value);
                }

                let data = if result.is_error {
                    serde_json::json!({"error": true, "content": content})
                } else {
                    serde_json::json!(content)
                };

                Ok(ToolResult {
                    data,
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                })
            }
            Err(e) => Err(ToolError::ExecutionFailed {
                message: format!(
                    "MCP tool call failed: {}.{}: {e}",
                    self.info.server_name, self.info.tool_name
                ),
                display_data: None,
                source: None,
            }),
        }
    }
}

async fn mcp_binary_block_for_model(
    ctx: &ToolUseContext,
    tool_name: &str,
    idx: usize,
    block_count: usize,
    blob: &str,
    mime_type: Option<&str>,
) -> Value {
    let Some(output_id) = mcp_binary_output_id(ctx, idx, block_count) else {
        return serde_json::json!({
            "type": "text",
            "text": format!("[Binary MCP content from {tool_name}: persistence unavailable]"),
        });
    };
    match persist_mcp_blob_reference(ctx, &output_id, blob, mime_type).await {
        Ok(reference) => serde_json::json!({"type": "text", "text": reference}),
        Err(message) => serde_json::json!({
            "type": "text",
            "text": format!("[Binary MCP content from {tool_name}: {message}]"),
        }),
    }
}

async fn read_mcp_resource_content_for_model(
    ctx: &ToolUseContext,
    content: &coco_tool_runtime::mcp_handle::McpResourceContent,
    idx: usize,
    total: usize,
) -> Value {
    if let Some(text) = &content.text {
        return serde_json::json!({
            "uri": content.uri,
            "text": text,
            "mime_type": content.mime_type,
        });
    }
    if let Some(blob) = &content.blob {
        if let Some(output_id) = mcp_binary_output_id(ctx, idx, total) {
            return match persist_mcp_blob_reference(
                ctx,
                &output_id,
                blob,
                content.mime_type.as_deref(),
            )
            .await
            {
                Ok(replacement) => serde_json::json!({
                    "uri": content.uri,
                    "mime_type": content.mime_type,
                    "persisted_output": replacement,
                }),
                Err(message) => serde_json::json!({
                    "uri": content.uri,
                    "mime_type": content.mime_type,
                    "has_blob": true,
                    "persistence_error": message,
                }),
            };
        }
        return serde_json::json!({
            "uri": content.uri,
            "mime_type": content.mime_type,
            "has_blob": true,
        });
    }
    serde_json::json!({
        "uri": content.uri,
        "mime_type": content.mime_type,
        "has_blob": false,
    })
}

fn mcp_binary_output_id(ctx: &ToolUseContext, idx: usize, total: usize) -> Option<String> {
    ctx.tool_use_id.as_deref().map(|tool_use_id| {
        if total == 1 {
            tool_use_id.to_string()
        } else {
            format!("{tool_use_id}-{}", idx + 1)
        }
    })
}

async fn persist_mcp_blob_reference(
    ctx: &ToolUseContext,
    output_id: &str,
    blob: &str,
    mime_type: Option<&str>,
) -> Result<String, String> {
    use base64::Engine as _;

    let session_dir = ctx
        .tool_result_session_dir
        .as_ref()
        .ok_or_else(|| "persistence unavailable".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .map_err(|e| format!("invalid base64 data: {e}"))?;
    let persisted =
        tool_result_storage::persist_mcp_binary_to_disk(session_dir, output_id, &bytes, mime_type)
            .await
            .map_err(|e| format!("failed to persist binary output: {e}"))?;
    Ok(tool_result_storage::render_mcp_binary_reference(&persisted))
}

#[cfg(test)]
#[path = "mcp_tools.test.rs"]
mod tests;
