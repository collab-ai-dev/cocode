# coco-mcp

MCP server lifecycle, config, auth, discovery, naming, channel permissions. Delegates wire protocol to `coco-rmcp-client` via the `rmcp` SDK.

## Key Types

- Connections: `McpConnectionManager`, `McpConnectionState`, `McpClientError`, `ConnectedMcpServer`
- Config: `McpConfigLoader`, `McpServerConfig`, `ScopedMcpServerConfig`, `ConfigScope`, `McpTransport`, `McpConfigChanged`, `watch_mcp_configs`, `DefinedMcpServer`, `defined_servers` (the single definition merge; the loader and `/mcp list` both derive from it), `entry_is_legacy_disabled` (removed `"disabled"` field → fail-safe off)
- Activation (`activation`): `McpActivation` (`Active` / `UserDisabled` / `AwaitingApproval` / `PolicyDenied` / `LegacyDisabled`), `McpActivationPolicy` (combines per-project user toggles from `GlobalConfig.projects` with the settings-derived `coco_config::McpPolicyConfig`; `filter_active` is the connection-path choke point for file **and** plugin servers), `project_key`. Definitions may come from the repo; nothing that *activates* one may. Repo-defined (Project-scope) servers fail closed until approved.
- Discovery: `DiscoveryCache`, `DiscoveredTool`, `DiscoveredResource`, `DynamicResourceQuery`, `ServerCapabilities`, `McpCapabilities`, `McpResource`, `McpToolDefinition`, `ToolAnnotations`, `discover_all`, `discover_tools_from_server`, `discover_resources`, `discover_resources_matching`, `refresh_server_capabilities`
- Auth: `OAuthConfig`, `OAuthTokens`, `OAuthTokenStore`
- Client-hosted MCP: `ClientRouteMessage`, `ClientRouteFuture`,
  `McpServerConfig::ClientHosted`, `McpClientHostedConfig`
- Channels: `ChannelPermission`, `ChannelPermissionRelay`, `DenyAllRelay`, `StaticPermissionRelay`
- Elicitation: `ElicitationRequest`, `ElicitationResult`, `ElicitationField`, `ElicitationFieldType`, `ElicitationMode`, `ElicitationType`, `ElicitResult`
- Naming: `mcp_tool_id`, `parse_mcp_tool_id`
- Tool call: `tool_call` module
- Re-exports from `coco-rmcp-client`: `RmcpClient`, `ElicitationResponse`, `McpAuthStatus`, `SendElicitation`

## Note

`coco-mcp` only owns coco-specific business logic (scopes, discovery caching, file watching, naming). All rmcp protocol details (state machine, transport, OAuth persistor) live in `coco-rmcp-client`.
Client-hosted MCP is modeled as a normal MCP transport whose JSON-RPC messages
are routed through an injected client callback; SDK JSON-RPC correlation stays
in the SDK adapter.
