# coco-mcp

MCP server lifecycle, config, auth, discovery, naming, channel permissions. Delegates wire protocol to `coco-rmcp-client` via the `rmcp` SDK.

## Key Types

- Connections: `McpConnectionManager`, `McpConnectionState`, `McpClientError`, `ConnectedMcpServer`
- Config: `McpConfigLoader`, `McpServerConfig`, `ScopedMcpServerConfig`, `ConfigScope`, `McpTransport`, `McpConfigChanged`, `watch_mcp_configs`
- Discovery: `DiscoveryCache`, `DiscoveredTool`, `DiscoveredResource`, `DynamicResourceQuery`, `ServerCapabilities`, `McpCapabilities`, `McpResource`, `McpToolDefinition`, `ToolAnnotations`, `discover_all`, `discover_tools_from_server`, `discover_resources`, `discover_resources_matching`, `refresh_server_capabilities`
- Auth: `OAuthConfig`, `OAuthTokens`, `OAuthTokenStore`
- Channels: `ChannelPermission`, `ChannelPermissionRelay`, `DenyAllRelay`, `StaticPermissionRelay`
- Elicitation: `ElicitationRequest`, `ElicitationResult`, `ElicitationField`, `ElicitationFieldType`, `ElicitationMode`, `ElicitationType`, `ElicitResult`
- Naming: `mcp_tool_id`, `parse_mcp_tool_id`
- Tool call: `tool_call` module
- Re-exports from `coco-rmcp-client`: `RmcpClient`, `ElicitationResponse`, `McpAuthStatus`, `SendElicitation`

## Note

`coco-mcp` only owns coco-specific business logic (scopes, discovery caching, file watching, naming). All rmcp protocol details (state machine, transport, OAuth persistor) live in `coco-rmcp-client`.
