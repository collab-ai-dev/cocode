//! MCP client lifecycle management.
//!
//! Wraps `coco-rmcp-client::RmcpClient` which provides actual MCP protocol
//! communication via the `rmcp` SDK (stdio + HTTP/SSE transports, OAuth,
//! session recovery).

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_error::BoxedError;
use coco_mcp_types::CallToolRequestParams;
use coco_mcp_types::CallToolResult;
use coco_mcp_types::InitializeResult;
use coco_mcp_types::JSONRPC_VERSION;
use coco_mcp_types::ListPromptsResult;
use coco_mcp_types::ListResourcesResult;
use coco_mcp_types::ListToolsResult;
use coco_mcp_types::ReadResourceRequestParams;
use coco_mcp_types::ReadResourceResult;
use coco_rmcp_client::McpAuthStatus;
use coco_rmcp_client::OAuthCredentialsStoreMode;
use coco_rmcp_client::RmcpClient;
use coco_rmcp_client::SendElicitation;
use coco_rmcp_client::determine_streamable_http_auth_status;
use coco_rmcp_client::perform_oauth_login_return_url;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::info;
use tracing::warn;

use crate::naming;
use crate::types::ConnectedMcpServer;
use crate::types::McpCapabilities;
use crate::types::McpConnectionState;
use crate::types::McpOAuthConfig;
use crate::types::McpPrompt;
use crate::types::McpResource;
use crate::types::McpServerConfig;
use crate::types::McpToolDefinition;
use crate::types::ScopedMcpServerConfig;

/// Default MCP tool call timeout (ms) — ~27.8 hours.
const DEFAULT_TOOL_TIMEOUT_MS: u64 = 100_000_000;

/// Default MCP initialization timeout.
const DEFAULT_INIT_TIMEOUT: Duration = Duration::from_secs(60);

pub type SdkRouteFuture =
    Pin<Box<dyn Future<Output = std::result::Result<serde_json::Value, String>> + Send>>;
pub type SdkRouteMessage =
    Arc<dyn Fn(String, serde_json::Value) -> SdkRouteFuture + Send + Sync + 'static>;

/// MCP connection manager — manages lifecycle of all MCP server connections.
///
/// Internally delegates to `coco_rmcp_client::RmcpClient` for actual MCP
/// protocol communication (stdio, HTTP/SSE, OAuth, session recovery).
#[derive(Clone)]
pub struct McpConnectionManager {
    configs: HashMap<String, ScopedMcpServerConfig>,
    connections: Arc<RwLock<HashMap<String, McpConnectionState>>>,
    /// Holds the underlying rmcp clients, keyed by server name.
    rmcp_clients: Arc<RwLock<HashMap<String, Arc<RmcpClient>>>>,
    sdk_route_message: Option<SdkRouteMessage>,
    /// Monotonic counter for inner JSON-RPC `id` values used when
    /// bridging messages through `mcp/routeMessage`. Per-manager so two
    /// SDK MCP servers (or two concurrent `tools/call` invocations) get
    /// distinct ids. Wrapped in `Arc` so `Clone` impl shares the counter
    /// across cloned managers — they cooperate rather than restarting
    /// from 0 each clone.
    sdk_route_next_id: Arc<AtomicI64>,
    tool_timeout_ms: u64,
    config_home: PathBuf,
    /// Opt-in sink notified with a server name after a *background* reconnect
    /// attempt settles (post-OAuth login, or an `authenticate()`-triggered
    /// reconnect). The app layer listens and re-reconciles the `ToolRegistry`
    /// for that server — the manager itself can't (it must not depend on
    /// `coco-tools`). `OnceLock` because it is wired once at session bootstrap;
    /// `Arc` so cloned managers (e.g. inside `spawn_reconnect`) share it.
    reconnect_notifier: Arc<std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// 15-min on-disk cache of servers that recently required auth, used to
    /// skip doomed connect-401 probes within the window.
    auth_cache: crate::auth_cache::McpNeedsAuthCache,
}

impl McpConnectionManager {
    pub fn new(config_home: PathBuf) -> Self {
        let auth_cache = crate::auth_cache::McpNeedsAuthCache::new(&config_home);
        Self {
            configs: HashMap::new(),
            connections: Arc::new(RwLock::new(HashMap::new())),
            rmcp_clients: Arc::new(RwLock::new(HashMap::new())),
            sdk_route_message: None,
            sdk_route_next_id: Arc::new(AtomicI64::new(0)),
            tool_timeout_ms: DEFAULT_TOOL_TIMEOUT_MS,
            config_home,
            reconnect_notifier: Arc::new(std::sync::OnceLock::new()),
            auth_cache,
        }
    }

    /// Wire the sink notified after each background reconnect attempt settles
    /// (see [`Self::reconnect_notifier`]). Idempotent — a second call is a
    /// no-op (`OnceLock`). Called once at session bootstrap by the app layer,
    /// which owns the listener that re-registers tools into the `ToolRegistry`.
    pub fn set_reconnect_notifier(&self, tx: tokio::sync::mpsc::UnboundedSender<String>) {
        let _ = self.reconnect_notifier.set(tx);
    }

    /// Notify the app-layer listener that `server_name`'s connection state may
    /// have changed via a background reconnect, so it can reconcile the tool
    /// registry. No-op when no listener is wired (tests / SDK paths).
    fn notify_reconnect(&self, server_name: &str) {
        if let Some(tx) = self.reconnect_notifier.get() {
            let _ = tx.send(server_name.to_string());
        }
    }

    pub fn new_with_runtime_config(
        config_home: PathBuf,
        config: &coco_config::McpRuntimeConfig,
    ) -> Self {
        let mut manager = Self::new(config_home);
        if let Some(tool_timeout_ms) = config.tool_timeout_ms {
            manager.tool_timeout_ms = tool_timeout_ms.max(1) as u64;
        }
        manager
    }

    /// Register a server configuration.
    ///
    /// Also seeds `connections[name] = Pending` so a concurrent
    /// `mcp/status` query before `connect()` runs reports the server
    /// as `pending` rather than `disconnected`.
    /// `connect()` overwrites the entry on success / failure.
    ///
    /// Uses `try_write` because:
    ///
    /// 1. `register_server` may run inside an async context (e.g.
    ///    the `mcp/setServers` handler) — `blocking_write` would
    ///    panic the runtime.
    /// 2. The only concurrent contender for the connections lock is
    ///    `connect()`, which sets its own `Pending` state at the
    ///    start of its critical section
    ///    (`McpConnectionManager::connect`). If we lose the race for
    ///    the lock, `connect()` is already writing `Pending` — the
    ///    wire status is correct either way.
    ///
    /// So a missed `try_write` is **observably equivalent** to a
    /// successful one. No silent regression.
    pub fn register_server(&mut self, config: ScopedMcpServerConfig) {
        let name = config.name.clone();
        info!(server = %name, "registering MCP server");
        self.configs.insert(name.clone(), config);
        if let Ok(mut conns) = self.connections.try_write() {
            conns.entry(name).or_insert(McpConnectionState::Pending {
                reconnect_attempts: 0,
            });
        }
    }

    /// Register multiple server configurations.
    pub fn register_all(&mut self, configs: Vec<ScopedMcpServerConfig>) {
        for config in configs {
            self.register_server(config);
        }
    }

    /// Install the control-channel router used by SDK-hosted MCP servers.
    ///
    /// SDK MCP servers run in the SDK client process. The manager still owns
    /// lifecycle and tool catalog state, but JSON-RPC messages are forwarded
    /// through this callback instead of through a child process or HTTP
    /// transport.
    pub fn set_sdk_route_message(&mut self, route: SdkRouteMessage) {
        self.sdk_route_message = Some(route);
    }

    /// Connect to a server by name using `rmcp` SDK.
    pub async fn connect(
        &self,
        server_name: &str,
        send_elicitation: SendElicitation,
    ) -> Result<(), McpClientError> {
        let config =
            self.configs
                .get(server_name)
                .ok_or_else(|| McpClientError::ServerNotFound {
                    name: server_name.to_string(),
                })?;

        info!(server = %server_name, transport = ?config.config, "connecting to MCP server");

        // Set state to Pending
        {
            let mut conns = self.connections.write().await;
            conns.insert(
                server_name.to_string(),
                McpConnectionState::Pending {
                    reconnect_attempts: 0,
                },
            );
        }

        let result = self
            .do_connect(server_name, &config.config, send_elicitation)
            .await;

        match result {
            Ok(connected) => {
                {
                    let mut conns = self.connections.write().await;
                    conns.insert(
                        server_name.to_string(),
                        McpConnectionState::Connected(connected),
                    );
                }
                // Success evicts any stale needs-auth marker so a later
                // bootstrap doesn't skip this now-authenticated server.
                self.auth_cache.clear(server_name).await;
                Ok(())
            }
            Err(e) => {
                // Layer A: distinguish "server needs an OAuth login" from a hard
                // failure. Probe runs WITHOUT holding the connections lock (it
                // does network I/O). `NotLoggedIn` → NeedsAuth so the per-server
                // authenticate pseudo-tool is surfaced; anything else stays Failed
                // (retryable).
                let needs_auth = self.probe_needs_auth(server_name, &config.config).await;
                let state = if needs_auth {
                    McpConnectionState::NeedsAuth { auth_url: None }
                } else {
                    McpConnectionState::Failed {
                        error: e.to_string(),
                    }
                };
                self.connections
                    .write()
                    .await
                    .insert(server_name.to_string(), state);
                if needs_auth {
                    // Cache the 401 so subsequent connect cycles skip the probe
                    // within the TTL window.
                    self.auth_cache.set(server_name).await;
                }
                Err(e)
            }
        }
    }

    /// Whether `server_name` has a recent needs-auth marker still within the
    /// TTL window. Lets bootstrap skip a doomed connect.
    pub async fn is_needs_auth_cached(&self, server_name: &str) -> bool {
        self.auth_cache.is_cached(server_name).await
    }

    /// Probe whether a failed connect was actually "OAuth login required"
    /// rather than a hard error. Only OAuth-capable HTTP/SSE transports are
    /// probed; a `NotLoggedIn` verdict means the connect failed for lack of
    /// credentials. Every other verdict (tokens present but rejected,
    /// bearer-token, unsupported) or a probe error returns `false`, keeping the
    /// original `Failed` classification so transient network faults stay
    /// retryable.
    async fn probe_needs_auth(&self, server_name: &str, config: &McpServerConfig) -> bool {
        let Some((url, headers, headers_helper)) = oauth_login_target(config) else {
            return false;
        };
        let Ok(headers) = resolve_http_headers(server_name, &url, &headers, &headers_helper).await
        else {
            return false;
        };
        matches!(
            determine_streamable_http_auth_status(
                server_name,
                &url,
                /*bearer_token_env_var*/ None,
                Some(headers),
                /*env_http_headers*/ None,
                OAuthCredentialsStoreMode::Auto,
                &self.config_home,
            )
            .await,
            Ok(McpAuthStatus::NotLoggedIn)
        )
    }

    /// Whether an OAuth-capable HTTP/SSE server has stored discovery state but no
    /// usable token, meaning a connect attempt would 401. When true, the
    /// authenticate pseudo-tool is surfaced directly. Returns `false` for
    /// XAA-configured servers, which can silently re-auth from a cached IdP
    /// id_token and must still attempt the connect.
    pub fn needs_auth_without_connect(&self, server_name: &str) -> bool {
        let Some(config) = self.configs.get(server_name) else {
            return false;
        };
        let (url, oauth) = match &config.config {
            McpServerConfig::Sse(c) => (&c.url, c.oauth.as_ref()),
            McpServerConfig::Http(c) => (&c.url, c.oauth.as_ref()),
            _ => return false,
        };
        if oauth.and_then(|o| o.xaa.as_ref()).is_some() {
            return false;
        }
        let store = crate::auth::OAuthTokenStore::from_config_home(&self.config_home);
        crate::auth::has_discovery_but_no_token(&store, &crate::auth::server_key(server_name, url))
    }

    /// Force a server into `NeedsAuth` without attempting a connection — used
    /// when [`Self::needs_auth_without_connect`] determines a connect would 401.
    pub async fn mark_needs_auth(&self, server_name: &str) {
        self.connections.write().await.insert(
            server_name.to_string(),
            McpConnectionState::NeedsAuth { auth_url: None },
        );
    }

    /// Transport label + endpoint URL for a configured server, used to describe
    /// the per-server `authenticate` pseudo-tool surfaced for `NeedsAuth`
    /// servers. `None` when the server isn't registered.
    pub fn auth_descriptor(&self, server_name: &str) -> Option<(String, Option<String>)> {
        let config = self.configs.get(server_name)?;
        let descriptor = match &config.config {
            McpServerConfig::Stdio(_) => ("stdio".to_string(), None),
            McpServerConfig::Sse(c) => ("sse".to_string(), Some(c.url.clone())),
            McpServerConfig::Http(c) => ("http".to_string(), Some(c.url.clone())),
            McpServerConfig::WebSocket(c) => ("websocket".to_string(), Some(c.url.clone())),
            McpServerConfig::Sdk(_) => ("sdk".to_string(), None),
            McpServerConfig::ClaudeAiProxy(c) => {
                ("claudeai-proxy".to_string(), Some(c.url.clone()))
            }
        };
        Some(descriptor)
    }

    /// Create rmcp client and initialize based on transport type.
    async fn do_connect(
        &self,
        server_name: &str,
        config: &McpServerConfig,
        send_elicitation: SendElicitation,
    ) -> Result<ConnectedMcpServer, McpClientError> {
        let client = match config {
            McpServerConfig::Stdio(stdio) => {
                let program = OsString::from(&stdio.command);
                let args: Vec<OsString> = stdio.args.iter().map(OsString::from).collect();
                let env = if stdio.env.is_empty() {
                    None
                } else {
                    Some(stdio.env.clone())
                };
                RmcpClient::new_stdio_client(
                    program,
                    args,
                    env,
                    &[],
                    stdio.cwd.as_ref().map(PathBuf::from),
                )
                .await
                .map_err(|e| McpClientError::SpawnFailed {
                    message: format!("stdio spawn failed: {e}"),
                })?
            }
            McpServerConfig::Sse(sse) => {
                ensure_xaa_tokens(server_name, &sse.url, sse.oauth.as_ref(), &self.config_home)
                    .await?;
                let headers =
                    resolve_http_headers(server_name, &sse.url, &sse.headers, &sse.headers_helper)
                        .await?;
                RmcpClient::new_streamable_http_client(
                    server_name,
                    &sse.url,
                    /*bearer_token*/ None,
                    Some(headers),
                    /*env_http_headers*/ None,
                    OAuthCredentialsStoreMode::Auto,
                    self.config_home.clone(),
                )
                .await
                .map_err(|e| McpClientError::SpawnFailed {
                    message: format!("SSE connect failed: {e}"),
                })?
            }
            McpServerConfig::Http(http) => {
                ensure_xaa_tokens(
                    server_name,
                    &http.url,
                    http.oauth.as_ref(),
                    &self.config_home,
                )
                .await?;
                let headers = resolve_http_headers(
                    server_name,
                    &http.url,
                    &http.headers,
                    &http.headers_helper,
                )
                .await?;
                RmcpClient::new_streamable_http_client(
                    server_name,
                    &http.url,
                    /*bearer_token*/ None,
                    Some(headers),
                    /*env_http_headers*/ None,
                    OAuthCredentialsStoreMode::Auto,
                    self.config_home.clone(),
                )
                .await
                .map_err(|e| McpClientError::SpawnFailed {
                    message: format!("HTTP connect failed: {e}"),
                })?
            }
            McpServerConfig::Sdk(sdk) => {
                return self.do_connect_sdk(server_name, sdk).await;
            }
            _ => {
                return Err(McpClientError::UnsupportedTransport);
            }
        };

        // Initialize MCP handshake
        let init_params = coco_mcp_types::InitializeRequestParams {
            capabilities: coco_mcp_types::ClientCapabilities {
                experimental: None,
                roots: None,
                sampling: None,
                elicitation: None,
            },
            client_info: coco_mcp_types::Implementation {
                name: "coco".to_string(),
                title: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                user_agent: None,
            },
            protocol_version: "2024-11-05".to_string(),
        };

        let init_result = client
            .initialize(init_params, Some(DEFAULT_INIT_TIMEOUT), send_elicitation)
            .await
            .map_err(|e| McpClientError::SpawnFailed {
                message: format!("MCP initialization failed: {e}"),
            })?;

        // Discover tools
        let tools_result = client.list_tools(None, Some(DEFAULT_INIT_TIMEOUT)).await;
        let tools: Vec<McpToolDefinition> = match tools_result {
            Ok(result) => result
                .tools
                .into_iter()
                .map(|t| McpToolDefinition {
                    name: t.name,
                    description: t.description,
                    input_schema: serde_json::to_value(t.input_schema).unwrap_or_default(),
                })
                .collect(),
            Err(e) => {
                warn!(server = %server_name, "failed to list tools: {e}");
                Vec::new()
            }
        };

        let client = Arc::new(client);
        {
            let mut clients = self.rmcp_clients.write().await;
            clients.insert(server_name.to_string(), Arc::clone(&client));
        }

        let caps = &init_result.capabilities;
        let capabilities = McpCapabilities {
            tools: !tools.is_empty(),
            resources: caps.resources.is_some(),
            prompts: caps.prompts.is_some(),
            channel: false,
            channel_permission: false,
        };

        // Discover resources and prompts when the server advertises them
        // (tools/list + resources/list + prompts/list fan-out). Prompts surface
        // as MCP slash-commands; resources back the List/Read MCP resource tools.
        let resources = if capabilities.resources {
            fetch_resources(&client, server_name).await
        } else {
            Vec::new()
        };
        let commands = if capabilities.prompts {
            fetch_prompts(&client, server_name).await
        } else {
            Vec::new()
        };

        info!(
            server = %server_name,
            tools = tools.len(),
            resources = resources.len(),
            prompts = commands.len(),
            "MCP server connected and initialized"
        );

        Ok(ConnectedMcpServer {
            name: server_name.to_string(),
            capabilities,
            // Server instructions are capped at 2048 chars (same limit as tool
            // descriptions) because they reach the model via a <system-reminder>.
            instructions: init_result
                .instructions
                .map(|s| crate::tool_call::truncate_description(&s)),
            tools,
            resources,
            commands,
        })
    }

    /// Call an MCP tool on a connected server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpClientError> {
        if let Some(config) = self.configs.get(server_name)
            && matches!(config.config, McpServerConfig::Sdk(_))
        {
            return self.call_sdk_tool(server_name, tool_name, arguments).await;
        }

        let clients = self.rmcp_clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| McpClientError::ServerNotFound {
                name: server_name.to_string(),
            })?;

        let timeout = Duration::from_millis(self.tool_timeout_ms);
        client
            .call_tool(tool_name.to_string(), arguments, Some(timeout))
            .await
            .map_err(|e| McpClientError::ToolCallFailed {
                message: e.to_string(),
            })
    }

    async fn do_connect_sdk(
        &self,
        server_name: &str,
        sdk: &crate::types::McpSdkConfig,
    ) -> Result<ConnectedMcpServer, McpClientError> {
        let route = self
            .sdk_route_message
            .as_ref()
            .cloned()
            .ok_or(McpClientError::UnsupportedTransport)?;

        let init = route_sdk_jsonrpc(
            &route,
            server_name,
            self.sdk_route_next_id.fetch_add(1, Ordering::Relaxed),
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "coco",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await?;
        let init_result: InitializeResult = parse_sdk_jsonrpc_result(init)?;

        // MCP notifications have no result. Older SDK-side shims may still
        // return an ack; either way the notification should not block connect.
        let _ = route(
            server_name.to_string(),
            serde_json::json!({
                "jsonrpc": JSONRPC_VERSION,
                "method": "notifications/initialized",
            }),
        )
        .await;

        let tools_response = route_sdk_jsonrpc(
            &route,
            server_name,
            self.sdk_route_next_id.fetch_add(1, Ordering::Relaxed),
            "tools/list",
            serde_json::json!({}),
        )
        .await?;
        let tools_result: ListToolsResult = parse_sdk_jsonrpc_result(tools_response)?;
        let tools = tools_result
            .tools
            .into_iter()
            .map(|t| McpToolDefinition {
                name: t.name,
                description: t.description,
                input_schema: serde_json::to_value(t.input_schema).unwrap_or_default(),
            })
            .collect::<Vec<_>>();

        let caps = &init_result.capabilities;
        let capabilities = McpCapabilities {
            tools: !tools.is_empty(),
            resources: caps.resources.is_some(),
            prompts: caps.prompts.is_some(),
            channel: false,
            channel_permission: false,
        };

        // Fetch resources + prompts when the server advertises them,
        // routing through the SDK control channel.
        let resources = if capabilities.resources {
            self.fetch_resources_for_sdk(&route, server_name).await
        } else {
            Vec::new()
        };
        let commands = if capabilities.prompts {
            self.fetch_prompts_for_sdk(&route, server_name).await
        } else {
            Vec::new()
        };

        info!(
            server = %server_name,
            sdk_name = %sdk.name,
            tools = tools.len(),
            resources = resources.len(),
            prompts = commands.len(),
            "SDK MCP server connected and initialized"
        );

        Ok(ConnectedMcpServer {
            name: server_name.to_string(),
            capabilities,
            // Server instructions are capped at 2048 chars (same limit as tool
            // descriptions) because they reach the model via a <system-reminder>.
            instructions: init_result
                .instructions
                .map(|s| crate::tool_call::truncate_description(&s)),
            tools,
            resources,
            commands,
        })
    }

    /// List an SDK-hosted server's resources via the control channel,
    /// mapping into [`McpResource`]. Parallels [`fetch_resources`] (rmcp path):
    /// a failure is logged and treated as "no resources" — it must not abort
    /// connect.
    async fn fetch_resources_for_sdk(
        &self,
        route: &SdkRouteMessage,
        server_name: &str,
    ) -> Vec<McpResource> {
        let response = match route_sdk_jsonrpc(
            route,
            server_name,
            self.sdk_route_next_id.fetch_add(1, Ordering::Relaxed),
            "resources/list",
            serde_json::json!({}),
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                warn!(server = %server_name, "failed to list SDK MCP resources: {e}");
                return Vec::new();
            }
        };
        match parse_sdk_jsonrpc_result::<ListResourcesResult>(response) {
            Ok(result) => result
                .resources
                .into_iter()
                .map(|r| McpResource {
                    uri: r.uri,
                    name: r.name,
                    description: r.description,
                    mime_type: r.mime_type,
                })
                .collect(),
            Err(e) => {
                warn!(server = %server_name, "failed to parse SDK MCP resources: {e}");
                Vec::new()
            }
        }
    }

    /// List an SDK-hosted server's prompts via the control channel, mapping
    /// into [`McpPrompt`] (surfaced as MCP slash-commands). Parallels
    /// [`fetch_prompts`] (rmcp path); failures are logged and treated as
    /// "no prompts".
    async fn fetch_prompts_for_sdk(
        &self,
        route: &SdkRouteMessage,
        server_name: &str,
    ) -> Vec<McpPrompt> {
        let response = match route_sdk_jsonrpc(
            route,
            server_name,
            self.sdk_route_next_id.fetch_add(1, Ordering::Relaxed),
            "prompts/list",
            serde_json::json!({}),
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                warn!(server = %server_name, "failed to list SDK MCP prompts: {e}");
                return Vec::new();
            }
        };
        match parse_sdk_jsonrpc_result::<ListPromptsResult>(response) {
            Ok(result) => result
                .prompts
                .into_iter()
                .map(|p| McpPrompt {
                    name: p.name,
                    description: p.description,
                })
                .collect(),
            Err(e) => {
                warn!(server = %server_name, "failed to parse SDK MCP prompts: {e}");
                Vec::new()
            }
        }
    }

    async fn call_sdk_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpClientError> {
        let route = self
            .sdk_route_message
            .as_ref()
            .cloned()
            .ok_or(McpClientError::UnsupportedTransport)?;
        let response = route_sdk_jsonrpc(
            &route,
            server_name,
            self.sdk_route_next_id.fetch_add(1, Ordering::Relaxed),
            "tools/call",
            serde_json::to_value(CallToolRequestParams {
                name: tool_name.to_string(),
                arguments,
            })
            .map_err(|e| McpClientError::ToolCallFailed {
                message: format!("serialize SDK MCP tools/call: {e}"),
            })?,
        )
        .await?;
        parse_sdk_jsonrpc_result(response)
    }

    /// Read an MCP resource from a connected server.
    pub async fn read_resource(
        &self,
        server_name: &str,
        resource_uri: &str,
    ) -> Result<ReadResourceResult, McpClientError> {
        let clients = self.rmcp_clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| McpClientError::ServerNotFound {
                name: server_name.to_string(),
            })?;

        let timeout = Duration::from_millis(self.tool_timeout_ms);
        client
            .read_resource(
                ReadResourceRequestParams {
                    uri: resource_uri.to_string(),
                },
                Some(timeout),
            )
            .await
            .map_err(|e| McpClientError::ToolCallFailed {
                message: format!("resource read failed: {e}"),
            })
    }

    /// Start or refresh OAuth authentication for a server.
    ///
    /// For OAuth-capable HTTP/SSE servers without stored tokens this
    /// returns the authorization URL immediately and reconnects in the
    /// background after the local callback completes.
    pub async fn authenticate(
        &self,
        server_name: &str,
        send_elicitation: SendElicitation,
    ) -> Result<String, McpClientError> {
        let config = self.configs.get(server_name).cloned().ok_or_else(|| {
            McpClientError::ServerNotFound {
                name: server_name.to_string(),
            }
        })?;
        let Some((url, headers, headers_helper)) = oauth_login_target(&config.config) else {
            return Ok(format!(
                "MCP server '{server_name}' does not use OAuth authentication."
            ));
        };
        let headers = resolve_http_headers(server_name, &url, &headers, &headers_helper).await?;

        let status = determine_streamable_http_auth_status(
            server_name,
            &url,
            /*bearer_token_env_var*/ None,
            Some(headers.clone()),
            /*env_http_headers*/ None,
            OAuthCredentialsStoreMode::Auto,
            &self.config_home,
        )
        .await
        .map_err(|e| McpClientError::ToolCallFailed {
            message: format!("failed to determine MCP auth status: {e}"),
        })?;

        match status {
            McpAuthStatus::Unsupported => Ok(format!(
                "MCP server '{server_name}' does not support OAuth authentication."
            )),
            McpAuthStatus::BearerToken => Ok(format!(
                "MCP server '{server_name}' is configured with bearer-token authentication; no OAuth login is needed."
            )),
            McpAuthStatus::OAuth => {
                self.spawn_reconnect(server_name.to_string(), send_elicitation);
                Ok(format!(
                    "OAuth credentials are already available for MCP server '{server_name}'. Reconnecting in the background."
                ))
            }
            McpAuthStatus::NotLoggedIn => {
                let handle = perform_oauth_login_return_url(
                    server_name,
                    &url,
                    OAuthCredentialsStoreMode::Auto,
                    Some(headers),
                    /*env_http_headers*/ None,
                    &[],
                    /*timeout_secs*/ None,
                    /*callback_port*/ None,
                    self.config_home.clone(),
                )
                .await
                .map_err(|e| McpClientError::ToolCallFailed {
                    message: format!("failed to start MCP OAuth login: {e}"),
                })?;
                let authorization_url = handle.authorization_url().to_string();
                self.spawn_reconnect_after_oauth(
                    server_name.to_string(),
                    handle,
                    send_elicitation,
                    authorization_url.clone(),
                );
                Ok(format!(
                    "Authentication started for MCP server '{server_name}'. Open this URL in your browser to continue:\n{authorization_url}\nThe server will reconnect automatically after OAuth completes."
                ))
            }
        }
    }

    fn spawn_reconnect(&self, server_name: String, send_elicitation: SendElicitation) {
        let manager = self.clone();
        tokio::spawn(async move {
            // Clear the needs-auth marker first so this reconnect isn't itself
            // skipped by a concurrent cache check.
            manager.auth_cache.clear(&server_name).await;
            if let Err(error) = manager.connect(&server_name, send_elicitation).await {
                warn!(server = %server_name, error = %error, "MCP reconnect after auth failed");
            }
            // Let the app layer re-reconcile the tool registry for this server
            // (install real tools on success, re-surface the auth tool if the
            // reconnect itself landed back in NeedsAuth).
            manager.notify_reconnect(&server_name);
        });
    }

    fn spawn_reconnect_after_oauth(
        &self,
        server_name: String,
        handle: coco_rmcp_client::OauthLoginHandle,
        send_elicitation: SendElicitation,
        authorization_url: String,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            if let Err(error) = handle.wait().await {
                warn!(server = %server_name, error = %error, "MCP OAuth login failed");
                {
                    let mut conns = manager.connections.write().await;
                    conns.insert(
                        server_name.clone(),
                        McpConnectionState::NeedsAuth {
                            auth_url: Some(authorization_url),
                        },
                    );
                }
                // Re-surface the auth pseudo-tool: the login failed, so the
                // server is back in NeedsAuth and the model should be able to
                // retry rather than be left tool-less.
                manager.notify_reconnect(&server_name);
                return;
            }
            // OAuth succeeded — clear the needs-auth marker before reconnecting
            // so the attempt proceeds.
            manager.auth_cache.clear(&server_name).await;
            if let Err(error) = manager.connect(&server_name, send_elicitation).await {
                warn!(server = %server_name, error = %error, "MCP reconnect after OAuth login failed");
            }
            manager.notify_reconnect(&server_name);
        });
    }

    /// Call an MCP tool using its full wire name (mcp__server__tool).
    pub async fn call_tool_by_wire_name(
        &self,
        wire_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult, McpClientError> {
        let (server, tool) =
            naming::parse_mcp_tool_id(wire_name).ok_or_else(|| McpClientError::ToolCallFailed {
                message: format!("invalid MCP tool name: {wire_name}"),
            })?;
        self.call_tool(&server, &tool, arguments).await
    }

    /// Get the current state of a server connection.
    /// Return the names of every registered server config, regardless
    /// of connection state. Used by external code (e.g. the SDK
    /// `mcp/status` handler) to enumerate servers whose state can
    /// then be queried via [`Self::get_state`].
    pub fn registered_server_names(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }

    pub async fn get_state(&self, server_name: &str) -> Option<McpConnectionState> {
        let conns = self.connections.read().await;
        conns.get(server_name).cloned()
    }

    /// Get all connected servers.
    pub async fn connected_servers(&self) -> Vec<ConnectedMcpServer> {
        let conns = self.connections.read().await;
        conns
            .values()
            .filter_map(|state| {
                if let McpConnectionState::Connected(server) = state {
                    Some(server.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get all tools from all connected servers.
    pub async fn all_tools(&self) -> Vec<(String, McpToolDefinition)> {
        let servers = self.connected_servers().await;
        let mut tools = Vec::new();
        for server in &servers {
            for tool in &server.tools {
                tools.push((server.name.clone(), tool.clone()));
            }
        }
        tools
    }

    /// Disconnect a specific server.
    pub async fn disconnect(&self, server_name: &str) {
        let mut conns = self.connections.write().await;
        conns.remove(server_name);
        let mut clients = self.rmcp_clients.write().await;
        clients.remove(server_name);
        info!(server = %server_name, "disconnected MCP server");
    }

    /// Fully unregister a server: drop its config entry AND its live connection
    /// (state + rmcp client). Use when a server is removed for good — e.g. a
    /// plugin is disabled/uninstalled — so it no longer appears in
    /// [`Self::registered_server_names`] and can't be lazily reconnected.
    /// [`Self::disconnect`] only tears down the connection; this also forgets the
    /// config.
    pub async fn unregister_server(&mut self, server_name: &str) {
        self.configs.remove(server_name);
        self.disconnect(server_name).await;
    }

    /// Disconnect all servers.
    pub async fn disconnect_all(&self) {
        let mut conns = self.connections.write().await;
        let count = conns.len();
        conns.clear();
        let mut clients = self.rmcp_clients.write().await;
        clients.clear();
        info!("disconnected {count} MCP servers");
    }

    /// Get the tool call timeout in milliseconds.
    pub fn tool_timeout_ms(&self) -> u64 {
        self.tool_timeout_ms
    }

    /// Start watching MCP config files for changes.
    ///
    /// Returns a receiver that emits events when `.mcp.json` files change.
    /// Caller should spawn a task to handle events (e.g. reload configs,
    /// reconnect servers).
    pub fn start_config_watcher(
        &self,
        project_root: Option<&PathBuf>,
    ) -> Result<tokio::sync::broadcast::Receiver<crate::config_watcher::McpConfigChanged>, BoxedError>
    {
        crate::config_watcher::watch_mcp_configs(
            &self.config_home,
            project_root.map(PathBuf::as_path),
        )
    }
}

/// List a connected server's resources, mapping into [`McpResource`].
/// A failure is logged and treated as "no resources" — it must not abort connect.
async fn fetch_resources(client: &RmcpClient, server_name: &str) -> Vec<McpResource> {
    match client
        .list_resources(None, Some(DEFAULT_INIT_TIMEOUT))
        .await
    {
        Ok(result) => result
            .resources
            .into_iter()
            .map(|r| McpResource {
                uri: r.uri,
                name: r.name,
                description: r.description,
                mime_type: r.mime_type,
            })
            .collect(),
        Err(e) => {
            warn!(server = %server_name, "failed to list MCP resources: {e}");
            Vec::new()
        }
    }
}

/// List a connected server's prompts, mapping into [`McpPrompt`] (surfaced as
/// MCP slash-commands). Failures are logged and treated as "no prompts".
async fn fetch_prompts(client: &RmcpClient, server_name: &str) -> Vec<McpPrompt> {
    match client.list_prompts(None, Some(DEFAULT_INIT_TIMEOUT)).await {
        Ok(result) => result
            .prompts
            .into_iter()
            .map(|p| McpPrompt {
                name: p.name,
                description: p.description,
            })
            .collect(),
        Err(e) => {
            warn!(server = %server_name, "failed to list MCP prompts: {e}");
            Vec::new()
        }
    }
}

async fn ensure_xaa_tokens(
    server_name: &str,
    url: &str,
    oauth: Option<&McpOAuthConfig>,
    config_home: &std::path::Path,
) -> Result<(), McpClientError> {
    let Some(oauth) = oauth else {
        return Ok(());
    };
    let Some(xaa) = &oauth.xaa else {
        return Ok(());
    };

    if coco_rmcp_client::has_valid_oauth_tokens(
        server_name,
        url,
        OAuthCredentialsStoreMode::Auto,
        config_home,
    )
    .map_err(|error| McpClientError::SpawnFailed {
        message: format!("failed to inspect stored OAuth credentials: {error}"),
    })? {
        info!(
            server = %server_name,
            "stored OAuth credentials found; skipping XAA exchange"
        );
        return Ok(());
    }

    let client_id = required_xaa_field(
        "oauth.clientId or oauth.xaa.clientId",
        oauth.client_id.as_ref().or(xaa.client_id.as_ref()),
    )?;
    let config = crate::xaa::XaaConfig {
        client_id: client_id.to_string(),
        client_secret: required_xaa_field("oauth.xaa.clientSecret", xaa.client_secret.as_ref())?
            .to_string(),
        idp_client_id: required_xaa_field("oauth.xaa.idpClientId", xaa.idp_client_id.as_ref())?
            .to_string(),
        idp_client_secret: xaa.idp_client_secret.clone(),
        idp_id_token: required_xaa_field("oauth.xaa.idpIdToken", xaa.idp_id_token.as_ref())?
            .to_string(),
        idp_token_endpoint: required_xaa_field(
            "oauth.xaa.idpTokenEndpoint",
            xaa.idp_token_endpoint.as_ref(),
        )?
        .to_string(),
        scope: xaa.scope.clone(),
    };

    let http_client = reqwest::Client::new();
    let result = crate::xaa::perform_cross_app_access(&http_client, url, &config)
        .await
        .map_err(|error| McpClientError::SpawnFailed {
            message: format!("XAA authentication failed: {error}"),
        })?;
    if !result.token_type.is_empty() && !result.token_type.eq_ignore_ascii_case("bearer") {
        warn!(
            server = %server_name,
            token_type = %result.token_type,
            "XAA returned non-bearer token type; persisting as OAuth bearer token"
        );
    }
    coco_rmcp_client::save_oauth_access_token(coco_rmcp_client::OAuthAccessTokenSave {
        server_name,
        url,
        client_id: &config.client_id,
        access_token: result.access_token,
        refresh_token: result.refresh_token,
        expires_in: result.expires_in,
        scopes: result.scope,
        store_mode: OAuthCredentialsStoreMode::Auto,
        config_home,
    })
    .map_err(|error| McpClientError::SpawnFailed {
        message: format!("failed to persist XAA credentials: {error}"),
    })
}

fn required_xaa_field<'a>(
    field: &str,
    value: Option<&'a String>,
) -> Result<&'a str, McpClientError> {
    value
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpClientError::SpawnFailed {
            message: format!("XAA config missing required field {field}"),
        })
}

fn oauth_login_target(
    config: &McpServerConfig,
) -> Option<(String, HashMap<String, String>, Option<String>)> {
    match config {
        McpServerConfig::Sse(sse) => Some((
            sse.url.clone(),
            sse.headers.clone(),
            sse.headers_helper.clone(),
        )),
        McpServerConfig::Http(http) => Some((
            http.url.clone(),
            http.headers.clone(),
            http.headers_helper.clone(),
        )),
        McpServerConfig::Stdio(_)
        | McpServerConfig::WebSocket(_)
        | McpServerConfig::Sdk(_)
        | McpServerConfig::ClaudeAiProxy(_) => None,
    }
}

async fn resolve_http_headers(
    server_name: &str,
    server_url: &str,
    static_headers: &HashMap<String, String>,
    helper: &Option<String>,
) -> Result<HashMap<String, String>, McpClientError> {
    let mut headers = static_headers.clone();
    let Some(helper) = helper.as_deref() else {
        return Ok(headers);
    };
    let dynamic = run_headers_helper(server_name, server_url, helper).await?;
    headers.extend(dynamic);
    Ok(headers)
}

async fn run_headers_helper(
    server_name: &str,
    server_url: &str,
    helper: &str,
) -> Result<HashMap<String, String>, McpClientError> {
    let mut cmd = shell_command(helper);
    cmd.env("CLAUDE_CODE_MCP_SERVER_NAME", server_name)
        .env("CLAUDE_CODE_MCP_SERVER_URL", server_url);
    let output = timeout(Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| McpClientError::SpawnFailed {
            message: format!("headersHelper timed out for MCP server '{server_name}'"),
        })?
        .map_err(|error| McpClientError::SpawnFailed {
            message: format!("headersHelper failed for MCP server '{server_name}': {error}"),
        })?;

    if !output.status.success() {
        return Err(McpClientError::SpawnFailed {
            message: format!(
                "headersHelper exited with status {} for MCP server '{}'",
                output.status, server_name
            ),
        });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|error| McpClientError::SpawnFailed {
        message: format!("headersHelper output was not UTF-8: {error}"),
    })?;
    parse_headers_helper_output(server_name, &stdout)
}

fn shell_command(helper: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(helper);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(helper);
        cmd
    }
}

fn parse_headers_helper_output(
    server_name: &str,
    stdout: &str,
) -> Result<HashMap<String, String>, McpClientError> {
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|error| McpClientError::SpawnFailed {
            message: format!("headersHelper returned invalid JSON for '{server_name}': {error}"),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| McpClientError::SpawnFailed {
            message: format!("headersHelper for '{server_name}' must return a JSON object"),
        })?;

    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        let Some(value) = value.as_str() else {
            return Err(McpClientError::SpawnFailed {
                message: format!(
                    "headersHelper for '{server_name}' returned non-string value for '{key}'"
                ),
            });
        };
        out.insert(key.clone(), value.to_string());
    }
    Ok(out)
}

async fn route_sdk_jsonrpc(
    route: &SdkRouteMessage,
    server_name: &str,
    id: i64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, McpClientError> {
    route(
        server_name.to_string(),
        serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await
    .map_err(|message| McpClientError::ToolCallFailed { message })
}

fn parse_sdk_jsonrpc_result<T>(message: serde_json::Value) -> Result<T, McpClientError>
where
    T: serde::de::DeserializeOwned,
{
    if let Some(error) = message.get("error") {
        return Err(McpClientError::ToolCallFailed {
            message: format!("SDK MCP returned error: {error}"),
        });
    }
    let result = message
        .get("result")
        .cloned()
        .ok_or_else(|| McpClientError::ToolCallFailed {
            message: format!("SDK MCP response missing result: {message}"),
        })?;
    serde_json::from_value(result).map_err(|e| McpClientError::ToolCallFailed {
        message: format!("parse SDK MCP response: {e}"),
    })
}

/// MCP client errors.
#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("MCP server not found: {name}")]
    ServerNotFound { name: String },
    #[error("failed to spawn MCP server: {message}")]
    SpawnFailed { message: String },
    #[error("unsupported transport type")]
    UnsupportedTransport,
    #[error("MCP session expired")]
    SessionExpired,
    #[error("MCP authentication required")]
    AuthRequired { auth_url: Option<String> },
    #[error("MCP tool call failed: {message}")]
    ToolCallFailed { message: String },
    #[error("MCP tool call timed out")]
    ToolCallTimeout,
}

// `McpClientError` keeps its `thiserror` shape (many existing call sites
// construct variants directly via `Self::Variant { .. }`); we layer the
// `coco-error` traits on top so callers can match on `StatusCode` and
// drive retry / classification without the mass-rewrite that a full
// snafu migration would require.
impl coco_error::StackError for McpClientError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for McpClientError {
    fn status_code(&self) -> coco_error::StatusCode {
        use coco_error::StatusCode;
        match self {
            Self::ServerNotFound { .. } => StatusCode::ProviderNotFound,
            Self::SpawnFailed { .. } => StatusCode::ConnectionFailed,
            Self::UnsupportedTransport => StatusCode::Unsupported,
            Self::SessionExpired => StatusCode::AuthenticationFailed,
            Self::AuthRequired { .. } => StatusCode::AuthenticationFailed,
            Self::ToolCallFailed { .. } => StatusCode::Internal,
            Self::ToolCallTimeout => StatusCode::Timeout,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
#[path = "client.test.rs"]
mod tests;
