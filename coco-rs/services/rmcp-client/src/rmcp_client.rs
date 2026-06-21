use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use coco_mcp_types::CallToolRequestParams;
use coco_mcp_types::CallToolResult;
use coco_mcp_types::InitializeRequestParams;
use coco_mcp_types::InitializeResult;
use coco_mcp_types::ListPromptsRequestParams;
use coco_mcp_types::ListPromptsResult;
use coco_mcp_types::ListResourceTemplatesRequestParams;
use coco_mcp_types::ListResourceTemplatesResult;
use coco_mcp_types::ListResourcesRequestParams;
use coco_mcp_types::ListResourcesResult;
use coco_mcp_types::ListToolsRequestParams;
use coco_mcp_types::ListToolsResult;
use coco_mcp_types::ReadResourceRequestParams;
use coco_mcp_types::ReadResourceResult;
use coco_mcp_types::RequestId;
use coco_mcp_types::Tool;
use futures::FutureExt;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::header::ACCEPT;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use rmcp::model::CallToolRequestParams as RmcpCallToolRequestParams;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::model::ClientNotification;
use rmcp::model::ClientRequest;
use rmcp::model::CreateElicitationRequestParams;
use rmcp::model::CreateElicitationResult;
use rmcp::model::CustomNotification;
use rmcp::model::CustomRequest;
use rmcp::model::Extensions;
use rmcp::model::InitializeRequestParams as RmcpInitializeRequestParams;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ReadResourceRequestParams as RmcpReadResourceRequestParams;
use rmcp::model::ServerResult;
use rmcp::service::RoleClient;
use rmcp::service::RunningService;
use rmcp::service::{self};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::auth::AuthClient;
use rmcp::transport::auth::OAuthState;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
use serde_json::Value;
use sse_stream::Sse;
use sse_stream::SseStream;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::Result;
use crate::RmcpClientError;
use crate::load_oauth_tokens;
use crate::logging_client_handler::LoggingClientHandler;
use crate::oauth::OAuthCredentialsStoreMode;
use crate::oauth::OAuthPersistor;
use crate::oauth::StoredOAuthTokens;
use crate::program_resolver;
use crate::utils::apply_default_headers;
use crate::utils::build_default_headers;
use crate::utils::convert_call_tool_result;
use crate::utils::convert_to_mcp;
use crate::utils::convert_to_rmcp;
use crate::utils::create_env_for_mcp_server;

// ============================================================================
// Custom StreamableHttpClient with typed session-expiry detection
// ============================================================================

const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
const JSON_MIME_TYPE: &str = "application/json";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";

/// Error type for the custom HTTP client that distinguishes session expiry
/// from ordinary reqwest errors.
#[derive(Debug, thiserror::Error)]
enum SessionAwareHttpError {
    #[error("streamable HTTP session expired with 404 Not Found")]
    SessionExpired404,
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
}

impl coco_error::StackError for SessionAwareHttpError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for SessionAwareHttpError {
    fn status_code(&self) -> coco_error::StatusCode {
        match self {
            Self::SessionExpired404 => coco_error::StatusCode::AuthenticationFailed,
            Self::Reqwest(_) => coco_error::StatusCode::NetworkError,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A reqwest-based HTTP client that detects HTTP 404 when a session ID is
/// present and maps it to [`SessionAwareHttpError::SessionExpired404`].
///
/// This enables typed error detection in [`RmcpClient::is_session_expired_404`]
/// via `downcast_ref` instead of fragile string matching.
#[derive(Clone)]
struct SessionAwareHttpClient {
    inner: reqwest::Client,
}

impl SessionAwareHttpClient {
    fn new(inner: reqwest::Client) -> Self {
        Self { inner }
    }

    fn wrap_reqwest_error(error: reqwest::Error) -> StreamableHttpError<SessionAwareHttpError> {
        StreamableHttpError::Client(SessionAwareHttpError::from(error))
    }
}

impl StreamableHttpClient for SessionAwareHttpClient {
    type Error = SessionAwareHttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(auth) = auth_header {
            request = request.bearer_auth(auth);
        }
        if let Some(sid) = session_id.as_ref() {
            request = request.header(HEADER_SESSION_ID, sid.as_ref());
        }
        for (name, value) in custom_headers {
            request = request.header(name, value);
        }

        let response = request
            .json(&message)
            .send()
            .await
            .map_err(Self::wrap_reqwest_error)?;

        // Detect session expiry: 404 with an active session.
        if response.status() == reqwest::StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::Client(
                SessionAwareHttpError::SessionExpired404,
            ));
        }

        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let response_session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(stream, response_session_id))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let msg = response.json().await.map_err(Self::wrap_reqwest_error)?;
                Ok(StreamableHttpPostResponse::Json(msg, response_session_id))
            }
            _ => {
                let _body = response
                    .error_for_status()
                    .map_err(Self::wrap_reqwest_error)?;
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<(), StreamableHttpError<Self::Error>> {
        let mut request = self.inner.delete(uri.as_ref());
        if let Some(auth) = auth_header {
            request = request.bearer_auth(auth);
        }
        for (name, value) in custom_headers {
            request = request.header(name, value);
        }
        let response = request
            .header(HEADER_SESSION_ID, session_id.as_ref())
            .send()
            .await
            .map_err(Self::wrap_reqwest_error)?;

        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }

        response
            .error_for_status()
            .map_err(Self::wrap_reqwest_error)?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<
        BoxStream<'static, std::result::Result<Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        let mut request = self
            .inner
            .get(uri.as_ref())
            .header(ACCEPT, EVENT_STREAM_MIME_TYPE)
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-Id", id);
        }
        if let Some(auth) = auth_header {
            request = request.bearer_auth(auth);
        }
        for (name, value) in custom_headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(Self::wrap_reqwest_error)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StreamableHttpError::Client(
                SessionAwareHttpError::SessionExpired404,
            ));
        }
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }

        let response = response
            .error_for_status()
            .map_err(Self::wrap_reqwest_error)?;

        let stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        Ok(stream)
    }
}

enum PendingTransport {
    ChildProcess(TokioChildProcess),
    StreamableHttp {
        transport: StreamableHttpClientTransport<SessionAwareHttpClient>,
    },
    StreamableHttpWithOAuth {
        transport: StreamableHttpClientTransport<AuthClient<SessionAwareHttpClient>>,
        oauth_persistor: OAuthPersistor,
    },
}

enum ClientState {
    Connecting {
        transport: Option<PendingTransport>,
    },
    Ready {
        service: Arc<RunningService<RoleClient, LoggingClientHandler>>,
        oauth: Option<OAuthPersistor>,
    },
}

/// Stores the constructor arguments so a transport can be recreated for
/// session recovery after an HTTP 404 (session expired).
#[derive(Clone)]
enum TransportRecipe {
    Stdio {
        program: OsString,
        args: Vec<OsString>,
        env: Option<HashMap<String, String>>,
        env_vars: Vec<String>,
        cwd: Option<PathBuf>,
    },
    StreamableHttp {
        server_name: String,
        url: String,
        bearer_token: Option<String>,
        http_headers: Option<HashMap<String, String>>,
        env_http_headers: Option<HashMap<String, String>>,
        store_mode: OAuthCredentialsStoreMode,
        config_home: PathBuf,
    },
}

/// Saved from `initialize()` so session recovery can re-handshake.
#[derive(Clone)]
struct InitializeContext {
    timeout: Option<Duration>,
    handler: LoggingClientHandler,
}

/// Distinguishes timeout from service errors inside `run_service_operation`.
#[derive(Debug, thiserror::Error)]
enum ClientOperationError {
    #[error(transparent)]
    Service(#[from] rmcp::service::ServiceError),
    #[error("timed out awaiting {label} after {duration:?}")]
    Timeout { label: String, duration: Duration },
}

impl coco_error::StackError for ClientOperationError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for ClientOperationError {
    fn status_code(&self) -> coco_error::StatusCode {
        match self {
            Self::Service(_) => coco_error::StatusCode::ProviderError,
            Self::Timeout { .. } => coco_error::StatusCode::Timeout,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl From<ClientOperationError> for RmcpClientError {
    fn from(err: ClientOperationError) -> Self {
        match err {
            ClientOperationError::Service(e) => Self::Service(e),
            ClientOperationError::Timeout { label, duration } => Self::Generic {
                message: format!("timed out awaiting {label} after {duration:?}"),
            },
        }
    }
}

pub type Elicitation = CreateElicitationRequestParams;
pub type ElicitationResponse = CreateElicitationResult;

/// Interface for sending elicitation requests to the UI and awaiting a response.
pub type SendElicitation = Box<
    dyn Fn(RequestId, Elicitation) -> BoxFuture<'static, Result<ElicitationResponse>> + Send + Sync,
>;

pub struct ToolWithConnectorId {
    pub tool: Tool,
    pub connector_id: Option<String>,
    pub connector_name: Option<String>,
}

pub struct ListToolsWithConnectorIdResult {
    pub next_cursor: Option<String>,
    pub tools: Vec<ToolWithConnectorId>,
}

/// MCP client implemented on top of the official `rmcp` SDK.
/// https://github.com/modelcontextprotocol/rust-sdk
pub struct RmcpClient {
    state: Mutex<ClientState>,
    transport_recipe: TransportRecipe,
    initialize_context: Mutex<Option<InitializeContext>>,
    /// Prevents concurrent session recovery attempts.
    session_recovery_lock: Mutex<()>,
}

impl RmcpClient {
    pub async fn new_stdio_client(
        program: OsString,
        args: Vec<OsString>,
        env: Option<HashMap<String, String>>,
        env_vars: &[String],
        cwd: Option<PathBuf>,
    ) -> io::Result<Self> {
        let recipe = TransportRecipe::Stdio {
            program,
            args,
            env,
            env_vars: env_vars.to_vec(),
            cwd,
        };
        let transport = Self::create_pending_transport(&recipe)
            .await
            .map_err(io::Error::other)?;

        Ok(Self {
            state: Mutex::new(ClientState::Connecting {
                transport: Some(transport),
            }),
            transport_recipe: recipe,
            initialize_context: Mutex::new(None),
            session_recovery_lock: Mutex::new(()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn new_streamable_http_client(
        server_name: &str,
        url: &str,
        bearer_token: Option<String>,
        http_headers: Option<HashMap<String, String>>,
        env_http_headers: Option<HashMap<String, String>>,
        store_mode: OAuthCredentialsStoreMode,
        config_home: PathBuf,
    ) -> Result<Self> {
        let recipe = TransportRecipe::StreamableHttp {
            server_name: server_name.to_string(),
            url: url.to_string(),
            bearer_token,
            http_headers,
            env_http_headers,
            store_mode,
            config_home,
        };
        let transport = Self::create_pending_transport(&recipe).await?;

        Ok(Self {
            state: Mutex::new(ClientState::Connecting {
                transport: Some(transport),
            }),
            transport_recipe: recipe,
            initialize_context: Mutex::new(None),
            session_recovery_lock: Mutex::new(()),
        })
    }

    /// Perform the initialization handshake with the MCP server.
    /// https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization
    pub async fn initialize(
        &self,
        params: InitializeRequestParams,
        timeout: Option<Duration>,
        send_elicitation: SendElicitation,
    ) -> Result<InitializeResult> {
        let rmcp_params: RmcpInitializeRequestParams = convert_to_rmcp(params.clone())?;
        let client_handler = LoggingClientHandler::new(rmcp_params, send_elicitation);

        // Save initialization context so session recovery can re-handshake.
        {
            let mut ctx = self.initialize_context.lock().await;
            *ctx = Some(InitializeContext {
                timeout,
                handler: client_handler.clone(),
            });
        }

        let (service, oauth_persistor) =
            Self::connect_pending_transport_from_state(&self.state, client_handler, timeout)
                .await?;

        let initialize_result_rmcp = service.peer().peer_info().ok_or_else(|| {
            RmcpClientError::generic("handshake succeeded but server info was missing")
        })?;
        let initialize_result: InitializeResult = convert_to_mcp(initialize_result_rmcp)?;

        {
            let mut guard = self.state.lock().await;
            *guard = ClientState::Ready {
                service: Arc::new(service),
                oauth: oauth_persistor.clone(),
            };
        }

        if let Some(runtime) = oauth_persistor
            && let Err(error) = runtime.persist_if_needed().await
        {
            warn!("failed to persist OAuth tokens after initialize: {error}");
        }

        info!(
            server_name = %initialize_result.server_info.name,
            protocol_version = %initialize_result.protocol_version,
            "MCP: initialized"
        );
        Ok(initialize_result)
    }

    pub async fn list_tools(
        &self,
        params: Option<ListToolsRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListToolsResult> {
        let result = self.list_tools_with_connector_ids(params, timeout).await?;
        Ok(ListToolsResult {
            next_cursor: result.next_cursor,
            tools: result.tools.into_iter().map(|tool| tool.tool).collect(),
        })
    }

    pub async fn list_tools_with_connector_ids(
        &self,
        params: Option<ListToolsRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListToolsWithConnectorIdResult> {
        self.refresh_oauth_if_needed().await;
        let rmcp_params = params
            .map(convert_to_rmcp::<_, PaginatedRequestParams>)
            .transpose()?;
        let result = self
            .run_service_operation("tools/list", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.list_tools(p).await }
                }
            })
            .await?;
        let tools = result
            .tools
            .into_iter()
            .map(|tool| {
                let meta = tool.meta.as_ref();
                let connector_id = Self::meta_string(meta, "connector_id");
                let connector_name = Self::meta_string(meta, "connector_name")
                    .or_else(|| Self::meta_string(meta, "connector_display_name"));
                let tool = convert_to_mcp(tool)?;
                Ok(ToolWithConnectorId {
                    tool,
                    connector_id,
                    connector_name,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.persist_oauth_tokens().await;
        Ok(ListToolsWithConnectorIdResult {
            next_cursor: result.next_cursor,
            tools,
        })
    }

    fn meta_string(meta: Option<&rmcp::model::Meta>, key: &str) -> Option<String> {
        meta.and_then(|meta| meta.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    pub async fn list_resources(
        &self,
        params: Option<ListResourcesRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListResourcesResult> {
        self.refresh_oauth_if_needed().await;
        let rmcp_params = params
            .map(convert_to_rmcp::<_, PaginatedRequestParams>)
            .transpose()?;
        let result = self
            .run_service_operation("resources/list", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.list_resources(p).await }
                }
            })
            .await?;
        let converted = convert_to_mcp(result)?;
        self.persist_oauth_tokens().await;
        Ok(converted)
    }

    pub async fn list_prompts(
        &self,
        params: Option<ListPromptsRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListPromptsResult> {
        self.refresh_oauth_if_needed().await;
        let rmcp_params = params
            .map(convert_to_rmcp::<_, PaginatedRequestParams>)
            .transpose()?;
        let result = self
            .run_service_operation("prompts/list", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.list_prompts(p).await }
                }
            })
            .await?;
        let converted = convert_to_mcp(result)?;
        self.persist_oauth_tokens().await;
        Ok(converted)
    }

    pub async fn list_resource_templates(
        &self,
        params: Option<ListResourceTemplatesRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListResourceTemplatesResult> {
        self.refresh_oauth_if_needed().await;
        let rmcp_params = params
            .map(convert_to_rmcp::<_, PaginatedRequestParams>)
            .transpose()?;
        let result = self
            .run_service_operation("resources/templates/list", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.list_resource_templates(p).await }
                }
            })
            .await?;
        let converted = convert_to_mcp(result)?;
        self.persist_oauth_tokens().await;
        Ok(converted)
    }

    pub async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        timeout: Option<Duration>,
    ) -> Result<ReadResourceResult> {
        self.refresh_oauth_if_needed().await;
        let rmcp_params: RmcpReadResourceRequestParams = convert_to_rmcp(params)?;
        let result = self
            .run_service_operation("resources/read", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.read_resource(p).await }
                }
            })
            .await?;
        let converted = convert_to_mcp(result)?;
        self.persist_oauth_tokens().await;
        Ok(converted)
    }

    #[tracing::instrument(skip(self, arguments), fields(tool = %name))]
    pub async fn call_tool(
        &self,
        name: String,
        arguments: Option<serde_json::Value>,
        timeout: Option<Duration>,
    ) -> Result<CallToolResult> {
        debug!(has_args = arguments.is_some(), "MCP: calling tool");
        let start = std::time::Instant::now();
        self.refresh_oauth_if_needed().await;
        let params = CallToolRequestParams {
            arguments,
            name: name.clone(),
        };
        let rmcp_params: RmcpCallToolRequestParams = convert_to_rmcp(params)?;
        let result = self
            .run_service_operation("tools/call", timeout, {
                let p = rmcp_params.clone();
                move |service| {
                    let p = p.clone();
                    async move { service.call_tool(p).await }
                }
            })
            .await?;
        let converted = convert_call_tool_result(result)?;
        self.persist_oauth_tokens().await;
        debug!(
            tool = %name,
            is_error = converted.is_error.unwrap_or(false),
            duration_ms = start.elapsed().as_millis() as i64,
            "MCP: tool call completed"
        );
        Ok(converted)
    }

    pub async fn send_custom_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let service: Arc<RunningService<RoleClient, LoggingClientHandler>> = self.service().await?;
        service
            .send_notification(ClientNotification::CustomNotification(CustomNotification {
                method: method.to_string(),
                params,
                extensions: Extensions::new(),
            }))
            .await?;
        Ok(())
    }

    pub async fn send_custom_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<ServerResult> {
        let service: Arc<RunningService<RoleClient, LoggingClientHandler>> = self.service().await?;
        let response = service
            .send_request(ClientRequest::CustomRequest(CustomRequest::new(
                method, params,
            )))
            .await?;
        Ok(response)
    }

    // ========================================================================
    // Session recovery infrastructure
    // ========================================================================

    /// Execute an operation on the MCP service. If the server returns HTTP 404
    /// (session expired), automatically recreates the transport, re-initializes
    /// the handshake, and retries the operation once.
    async fn run_service_operation<T, F, Fut>(
        &self,
        label: &str,
        timeout: Option<Duration>,
        operation: F,
    ) -> Result<T>
    where
        F: Fn(Arc<RunningService<RoleClient, LoggingClientHandler>>) -> Fut + Clone,
        Fut: std::future::Future<Output = std::result::Result<T, rmcp::service::ServiceError>>,
    {
        let service = self.service().await?;
        match Self::run_once(Arc::clone(&service), label, timeout, operation.clone()).await {
            Ok(result) => Ok(result),
            Err(error) if Self::is_session_expired_404(&error) => {
                info!("MCP session expired, attempting recovery for {label}");
                self.reinitialize_after_session_expiry(&service).await?;
                let recovered = self.service().await?;
                Self::run_once(recovered, label, timeout, operation)
                    .await
                    .map_err(Into::into)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Run a single service operation with an optional timeout.
    async fn run_once<T, F, Fut>(
        service: Arc<RunningService<RoleClient, LoggingClientHandler>>,
        label: &str,
        timeout: Option<Duration>,
        operation: F,
    ) -> std::result::Result<T, ClientOperationError>
    where
        F: FnOnce(Arc<RunningService<RoleClient, LoggingClientHandler>>) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, rmcp::service::ServiceError>>,
    {
        match timeout {
            Some(duration) => time::timeout(duration, operation(service))
                .await
                .map_err(|_| ClientOperationError::Timeout {
                    label: label.to_string(),
                    duration,
                })?
                .map_err(ClientOperationError::from),
            None => operation(service).await.map_err(ClientOperationError::from),
        }
    }

    /// Check if the error is an HTTP 404 indicating a stale session.
    ///
    /// Uses typed error detection via `downcast_ref` on the transport error
    /// to match the exact [`SessionAwareHttpError::SessionExpired404`] variant
    /// produced by our custom [`SessionAwareHttpClient`].
    fn is_session_expired_404(error: &ClientOperationError) -> bool {
        let ClientOperationError::Service(rmcp::service::ServiceError::TransportSend(err)) = error
        else {
            return false;
        };
        err.error
            .downcast_ref::<StreamableHttpError<SessionAwareHttpError>>()
            .is_some_and(|e| {
                matches!(
                    e,
                    StreamableHttpError::Client(SessionAwareHttpError::SessionExpired404)
                )
            })
    }

    /// Recreate the transport from the stored recipe, re-handshake, and swap
    /// the service reference. Uses `session_recovery_lock` to prevent
    /// concurrent recovery attempts.
    async fn reinitialize_after_session_expiry(
        &self,
        failed_service: &Arc<RunningService<RoleClient, LoggingClientHandler>>,
    ) -> Result<()> {
        let _guard = self.session_recovery_lock.lock().await;

        // Another task may have already recovered — check via pointer equality.
        {
            let state = self.state.lock().await;
            if let ClientState::Ready { service, .. } = &*state
                && !Arc::ptr_eq(service, failed_service)
            {
                return Ok(());
            }
        }

        let init_ctx = self
            .initialize_context
            .lock()
            .await
            .clone()
            .ok_or_else(|| RmcpClientError::generic("cannot recover before initialize succeeds"))?;

        let transport = Self::create_pending_transport(&self.transport_recipe).await?;

        // Put the new pending transport in state so connect_pending_transport
        // can take it.
        {
            let mut state = self.state.lock().await;
            *state = ClientState::Connecting {
                transport: Some(transport),
            };
        }

        let (service, oauth_persistor) = Self::connect_pending_transport_from_state(
            &self.state,
            init_ctx.handler,
            init_ctx.timeout,
        )
        .await?;

        {
            let mut state = self.state.lock().await;
            *state = ClientState::Ready {
                service: Arc::new(service),
                oauth: oauth_persistor.clone(),
            };
        }

        if let Some(runtime) = oauth_persistor
            && let Err(error) = runtime.persist_if_needed().await
        {
            warn!("failed to persist OAuth tokens after session recovery: {error}");
        }

        info!("MCP session recovery succeeded");
        Ok(())
    }

    /// Create a fresh `PendingTransport` from the stored recipe.
    async fn create_pending_transport(recipe: &TransportRecipe) -> Result<PendingTransport> {
        match recipe {
            TransportRecipe::Stdio {
                program,
                args,
                env,
                env_vars,
                cwd,
            } => {
                let program_name = program.to_string_lossy().into_owned();
                let envs = create_env_for_mcp_server(env.clone(), env_vars);
                let resolved_program = program_resolver::resolve(program.clone(), &envs)?;

                let mut command = Command::new(resolved_program);
                command
                    .kill_on_drop(true)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .env_clear()
                    .envs(envs)
                    .args(args);
                if let Some(cwd) = cwd {
                    command.current_dir(cwd);
                }

                let (transport, stderr) = TokioChildProcess::builder(command)
                    .stderr(Stdio::piped())
                    .spawn()?;

                if let Some(stderr) = stderr {
                    tokio::spawn(async move {
                        let mut reader = BufReader::new(stderr).lines();
                        loop {
                            match reader.next_line().await {
                                Ok(Some(line)) => {
                                    info!("MCP server stderr ({program_name}): {line}");
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    warn!(
                                        "Failed to read MCP server stderr ({program_name}): {error}"
                                    );
                                    break;
                                }
                            }
                        }
                    });
                }

                Ok(PendingTransport::ChildProcess(transport))
            }
            TransportRecipe::StreamableHttp {
                server_name,
                url,
                bearer_token,
                http_headers,
                env_http_headers,
                store_mode,
                config_home,
            } => {
                let default_headers =
                    build_default_headers(http_headers.clone(), env_http_headers.clone())?;

                let initial_oauth_tokens = match bearer_token {
                    Some(_) => None,
                    None => match load_oauth_tokens(server_name, url, *store_mode, config_home) {
                        Ok(tokens) => tokens,
                        Err(err) => {
                            warn!("failed to read tokens for server `{server_name}`: {err}");
                            None
                        }
                    },
                };

                if let Some(initial_tokens) = initial_oauth_tokens {
                    let (transport, oauth_persistor) = create_oauth_transport_and_runtime(
                        server_name,
                        url,
                        initial_tokens,
                        *store_mode,
                        default_headers,
                        config_home.clone(),
                    )
                    .await?;
                    Ok(PendingTransport::StreamableHttpWithOAuth {
                        transport,
                        oauth_persistor,
                    })
                } else {
                    let mut http_config =
                        StreamableHttpClientTransportConfig::with_uri(url.to_string());
                    if let Some(bearer_token) = bearer_token.clone() {
                        http_config = http_config.auth_header(bearer_token);
                    }
                    let reqwest_client =
                        apply_default_headers(reqwest::Client::builder(), &default_headers)
                            .build()?;
                    let http_client = SessionAwareHttpClient::new(reqwest_client);
                    let transport =
                        StreamableHttpClientTransport::with_client(http_client, http_config);
                    Ok(PendingTransport::StreamableHttp { transport })
                }
            }
        }
    }

    /// Take the pending transport from state, perform the MCP handshake, and
    /// return the running service (but do NOT update state — caller does that).
    async fn connect_pending_transport_from_state(
        state: &Mutex<ClientState>,
        client_handler: LoggingClientHandler,
        timeout: Option<Duration>,
    ) -> Result<(
        RunningService<RoleClient, LoggingClientHandler>,
        Option<OAuthPersistor>,
    )> {
        let (transport_fut, oauth_persistor) = {
            let mut guard = state.lock().await;
            match &mut *guard {
                ClientState::Connecting { transport } => match transport.take() {
                    Some(PendingTransport::ChildProcess(t)) => (
                        service::serve_client(client_handler.clone(), t).boxed(),
                        None,
                    ),
                    Some(PendingTransport::StreamableHttp { transport: t }) => (
                        service::serve_client(client_handler.clone(), t).boxed(),
                        None,
                    ),
                    Some(PendingTransport::StreamableHttpWithOAuth {
                        transport: t,
                        oauth_persistor,
                    }) => (
                        service::serve_client(client_handler.clone(), t).boxed(),
                        Some(oauth_persistor),
                    ),
                    None => {
                        return Err(RmcpClientError::InvalidState {
                            state: "already initializing",
                        });
                    }
                },
                ClientState::Ready { .. } => {
                    return Err(RmcpClientError::InvalidState {
                        state: "already initialized",
                    });
                }
            }
        };

        let service = match timeout {
            Some(duration) => time::timeout(duration, transport_fut)
                .await
                .map_err(|_| {
                    RmcpClientError::generic(format!(
                        "timed out handshaking with MCP server after {duration:?}"
                    ))
                })?
                .map_err(|err| {
                    RmcpClientError::generic(format!("handshaking with MCP server failed: {err}"))
                })?,
            None => transport_fut.await.map_err(|err| {
                RmcpClientError::generic(format!("handshaking with MCP server failed: {err}"))
            })?,
        };

        Ok((service, oauth_persistor))
    }

    // ========================================================================
    // State accessors
    // ========================================================================

    async fn service(&self) -> Result<Arc<RunningService<RoleClient, LoggingClientHandler>>> {
        let guard = self.state.lock().await;
        match &*guard {
            ClientState::Ready { service, .. } => Ok(Arc::clone(service)),
            ClientState::Connecting { .. } => Err(RmcpClientError::InvalidState {
                state: "not initialized",
            }),
        }
    }

    async fn oauth_persistor(&self) -> Option<OAuthPersistor> {
        let guard = self.state.lock().await;
        match &*guard {
            ClientState::Ready {
                oauth: Some(runtime),
                service: _,
            } => Some(runtime.clone()),
            _ => None,
        }
    }

    /// This should be called after every tool call so that if a given tool call triggered
    /// a refresh of the OAuth tokens, they are persisted.
    async fn persist_oauth_tokens(&self) {
        if let Some(runtime) = self.oauth_persistor().await
            && let Err(error) = runtime.persist_if_needed().await
        {
            warn!("failed to persist OAuth tokens: {error}");
        }
    }

    async fn refresh_oauth_if_needed(&self) {
        if let Some(runtime) = self.oauth_persistor().await
            && let Err(error) = runtime.refresh_if_needed().await
        {
            warn!("failed to refresh OAuth tokens: {error}");
        }
    }
}

async fn create_oauth_transport_and_runtime(
    server_name: &str,
    url: &str,
    initial_tokens: StoredOAuthTokens,
    credentials_store: OAuthCredentialsStoreMode,
    default_headers: HeaderMap,
    config_home: PathBuf,
) -> Result<(
    StreamableHttpClientTransport<AuthClient<SessionAwareHttpClient>>,
    OAuthPersistor,
)> {
    let reqwest_client =
        apply_default_headers(reqwest::Client::builder(), &default_headers).build()?;
    // OAuthState uses a raw reqwest::Client for OAuth discovery/token exchange.
    let mut oauth_state = OAuthState::new(url.to_string(), Some(reqwest_client.clone())).await?;

    oauth_state
        .set_credentials(
            &initial_tokens.client_id,
            initial_tokens.token_response.0.clone(),
        )
        .await?;

    let manager = match oauth_state {
        OAuthState::Authorized(manager) => manager,
        OAuthState::Unauthorized(manager) => manager,
        // OAuthState is #[non_exhaustive] as of rmcp 1.7: Session /
        // AuthorizedHttpClient and any future variants are unexpected here.
        _ => {
            return Err(RmcpClientError::InvalidState {
                state: "unexpected OAuth state during client setup",
            });
        }
    };

    // Wrap in SessionAwareHttpClient for MCP message transport.
    let http_client = SessionAwareHttpClient::new(reqwest_client);
    let auth_client = AuthClient::new(http_client, manager);
    let auth_manager = auth_client.auth_manager.clone();

    let transport = StreamableHttpClientTransport::with_client(
        auth_client,
        StreamableHttpClientTransportConfig::with_uri(url.to_string()),
    );

    let runtime = OAuthPersistor::new(
        server_name.to_string(),
        url.to_string(),
        auth_manager,
        credentials_store,
        config_home,
        Some(initial_tokens),
    );

    Ok((transport, runtime))
}
