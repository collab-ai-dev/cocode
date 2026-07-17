# coco-mcp

MCP server lifecycle, config, auth, discovery, naming, channel permissions. Delegates wire protocol to `coco-rmcp-client` via the `rmcp` SDK.

## Key Types

- Connections: `McpConnectionManager` (+ `McpConnectionState`, `ConnectedMcpServer`)
- Config: `McpConfigLoader`, `McpServerConfig` (incl. `::ClientHosted`), `ConfigScope`, `watch_mcp_configs`; `defined_servers` is the single definition merge — the loader and `/mcp list` both derive from it, so the views cannot disagree
- Activation (`activation`): `McpActivation` + `McpActivationPolicy`, which combines per-project user toggles from `GlobalConfig.projects` with the settings-derived `coco_config::McpPolicyConfig`. `filter_active` is the connection-path choke point for file **and** plugin servers. Definitions may come from the repo; nothing that *activates* one may — repo-defined (Project-scope) servers fail closed until approved, and a removed `"disabled"` field is fail-safe off (`entry_is_legacy_disabled`)
- Discovery: `DiscoveryCache` + `discover_*` functions
- Auth: `OAuthConfig`, `OAuthTokenStore`; elicitation types under `Elicitation*`
- Channels: `ChannelPermissionRelay` (+ `DenyAllRelay`, `StaticPermissionRelay`)
- Naming: `mcp_tool_id`, `parse_mcp_tool_id`

## Note

`coco-mcp` only owns coco-specific business logic (scopes, discovery caching, file watching, naming). All rmcp protocol details (state machine, transport, OAuth persistor) live in `coco-rmcp-client`.
Client-hosted MCP is modeled as a normal MCP transport whose JSON-RPC messages
are routed through an injected client callback; SDK JSON-RPC correlation stays
in the SDK adapter.
