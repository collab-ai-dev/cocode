#![allow(clippy::expect_used)]

use std::{future::Future, pin::Pin, sync::Arc};

use coco_agent_host::{
    AgentHostOptions,
    app_server_host::{
        AppServerHostHandler, AppServerHostState, HostInputs, OutboundMessage,
        RemoteJsonRpcConnection, RuntimeReplacementContext, TurnRunner,
        shutdown_local_app_server_sessions,
    },
    app_session::AppSessionHandle,
    local_client::LocalServerClient,
    session_runtime::{
        SessionRuntimeBootstrap, SessionRuntimeBootstrapSource, SessionRuntimeFactory,
        SessionRuntimeFactoryOpts,
    },
};
use coco_app_server::{
    AppServer, AttachSurfaceOptions, JsonRpcConnectionHandlerFactory, LocalClientAdapter,
    LocalClientSubscribeOutcome, SurfaceCapability,
};
use coco_app_server_client::ClientError;
use coco_app_server_transport::{JsonRpcFrame, JsonRpcId, JsonRpcNotification, JsonRpcRequest};
use coco_types::{
    ClientRequestMethod, ConfigWriteParams, ConfigWriteTarget, CoreEvent, HookCallbackMatcher,
    HookEventType, InitializeParams, InteractiveTarget, ServerNotification, SessionCloseParams,
    SessionCloseTarget, SessionDeleteParams, SessionEnvelope, SessionReadParams, SessionReadResult,
    SessionResumeParams, SessionResumeResult, SessionStartParams, SessionStartResult, SessionState,
    SessionTarget,
};
use tokio::sync::mpsc;

struct Fixture {
    _home: tempfile::TempDir,
    state: Arc<AppServerHostState>,
    server: Arc<AppServer<AppSessionHandle>>,
    adapter: LocalClientAdapter<AppSessionHandle>,
    handler: AppServerHostHandler,
    turn_observations: tokio::sync::Mutex<mpsc::UnboundedReceiver<TurnObservation>>,
    turn_gate: Option<Arc<TurnGate>>,
    observed_outbound: Arc<tokio::sync::Mutex<Vec<ObservedOutbound>>>,
    _outbound_collector: tokio::task::JoinHandle<()>,
}

async fn fixture() -> Fixture {
    fixture_with_turn_gate(None).await
}

#[derive(Debug, Clone, Copy)]
enum LifecycleConformanceSurface {
    LocalTyped,
    JsonRpc,
    #[cfg(unix)]
    UnixSidecar,
}

impl LifecycleConformanceSurface {
    fn label(self) -> &'static str {
        match self {
            Self::LocalTyped => "local_typed",
            Self::JsonRpc => "json_rpc",
            #[cfg(unix)]
            Self::UnixSidecar => "unix_sidecar",
        }
    }

    async fn connect(self, fixture: &Fixture) -> LifecycleConformanceClient {
        match self {
            Self::LocalTyped => LifecycleConformanceClient::LocalTyped {
                client: LocalServerClient::connect_local(&fixture.adapter),
            },
            Self::JsonRpc => {
                let adapter =
                    coco_agent_host::app_server_host::RemoteJsonRpcAdapter::with_channel_capacity(
                        Arc::clone(&fixture.server),
                        /*channel_capacity*/ 16,
                    );
                LifecycleConformanceClient::JsonRpc {
                    connection: adapter.connect(),
                    next_request_id: 1,
                }
            }
            #[cfg(unix)]
            Self::UnixSidecar => {
                let dir = tempfile::tempdir().expect("sidecar socket dir");
                let socket_path = dir.path().join("app-server.sock");
                let (outbound_tx, outbound_rx) = mpsc::channel(16);
                let handler = Arc::new(
                    AppServerHostHandler::with_local_app_server_and_turn_drain_timeout(
                        Arc::clone(&fixture.state),
                        outbound_tx,
                        Arc::clone(&fixture.server),
                        coco_agent_host::app_server_host::APP_SERVER_TURN_DRAIN_TIMEOUT,
                    ),
                );
                let outbound_forwarder =
                    coco_agent_host::app_server_host::spawn_app_server_local_outbound_forwarder(
                        Arc::clone(&fixture.server),
                        Arc::clone(&fixture.state),
                        outbound_rx,
                        Arc::new(std::sync::RwLock::new(None)),
                    );
                let adapter =
                    coco_agent_host::app_server_host::RemoteJsonRpcAdapter::with_channel_capacity(
                        Arc::clone(&fixture.server),
                        /*channel_capacity*/ 16,
                    );
                let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
                let listener_task = tokio::spawn({
                    let adapter = adapter.clone();
                    let handler = Arc::clone(&handler);
                    let socket_path = socket_path.clone();
                    async move {
                        adapter
                            .bind_and_run_unix_listener_until_shutdown(
                                socket_path,
                                handler,
                                shutdown_rx,
                            )
                            .await
                            .map_err(|error| error.to_string())
                    }
                });
                let mut transport =
                    tokio::time::timeout(std::time::Duration::from_secs(1), async {
                        loop {
                            match coco_app_server_transport::connect_ndjson_unix(&socket_path).await
                            {
                                Ok(transport) => break transport,
                                Err(_) => tokio::task::yield_now().await,
                            }
                        }
                    })
                    .await
                    .expect("sidecar listener starts");
                let mut next_request_id = 1;
                let mut notifications = Vec::new();
                let frame = request_over_ndjson_transport(
                    &mut transport,
                    &mut next_request_id,
                    &mut notifications,
                    ClientRequestMethod::Initialize,
                    InitializeParams::default(),
                    "unix sidecar initialize",
                )
                .await;
                json_rpc_success::<serde_json::Value>(frame, "unix sidecar initialize");
                LifecycleConformanceClient::UnixSidecar {
                    _socket_dir: dir,
                    transport,
                    shutdown_tx: Some(shutdown_tx),
                    listener_task,
                    outbound_forwarder,
                    next_request_id,
                    notifications,
                }
            }
        }
    }
}

enum LifecycleConformanceClient {
    LocalTyped {
        client: LocalServerClient<AppSessionHandle>,
    },
    JsonRpc {
        connection: RemoteJsonRpcConnection,
        next_request_id: i64,
    },
    #[cfg(unix)]
    UnixSidecar {
        _socket_dir: tempfile::TempDir,
        transport: coco_app_server_transport::NdjsonUnixConnection,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        listener_task: tokio::task::JoinHandle<Result<(), String>>,
        outbound_forwarder: tokio::task::JoinHandle<()>,
        next_request_id: i64,
        notifications: Vec<JsonRpcNotification>,
    },
}

impl LifecycleConformanceClient {
    async fn session_start(
        &mut self,
        fixture: &Fixture,
        params: SessionStartParams,
    ) -> SessionStartResult {
        match self {
            Self::LocalTyped { client } => client
                .session_start(&fixture.handler, params)
                .await
                .expect("local session/start"),
            Self::JsonRpc {
                connection,
                next_request_id,
            } => {
                let frame = connection
                    .dispatch_client_request(
                        json_rpc_request(
                            next_request_id,
                            ClientRequestMethod::SessionStart,
                            params,
                        ),
                        &fixture.handler,
                    )
                    .await;
                json_rpc_success(frame, "json-rpc session/start")
            }
            #[cfg(unix)]
            Self::UnixSidecar {
                transport,
                next_request_id,
                notifications,
                ..
            } => {
                let frame = request_over_ndjson_transport(
                    transport,
                    next_request_id,
                    notifications,
                    ClientRequestMethod::SessionStart,
                    params,
                    "unix sidecar session/start",
                )
                .await;
                json_rpc_success(frame, "unix sidecar session/start")
            }
        }
    }

    async fn session_read(
        &mut self,
        fixture: &Fixture,
        params: SessionReadParams,
    ) -> SessionReadResult {
        match self {
            Self::LocalTyped { client } => client
                .session_read(&fixture.handler, params)
                .await
                .expect("local session/read"),
            Self::JsonRpc {
                connection,
                next_request_id,
            } => {
                let frame = connection
                    .dispatch_client_request(
                        json_rpc_request(next_request_id, ClientRequestMethod::SessionRead, params),
                        &fixture.handler,
                    )
                    .await;
                json_rpc_success(frame, "json-rpc session/read")
            }
            #[cfg(unix)]
            Self::UnixSidecar {
                transport,
                next_request_id,
                notifications,
                ..
            } => {
                let frame = request_over_ndjson_transport(
                    transport,
                    next_request_id,
                    notifications,
                    ClientRequestMethod::SessionRead,
                    params,
                    "unix sidecar session/read",
                )
                .await;
                json_rpc_success(frame, "unix sidecar session/read")
            }
        }
    }

    async fn session_resume(
        &mut self,
        fixture: &Fixture,
        params: SessionResumeParams,
    ) -> SessionResumeResult {
        match self {
            Self::LocalTyped { client } => client
                .session_resume(&fixture.handler, params)
                .await
                .expect("local session/resume"),
            Self::JsonRpc {
                connection,
                next_request_id,
            } => {
                let frame = connection
                    .dispatch_client_request(
                        json_rpc_request(
                            next_request_id,
                            ClientRequestMethod::SessionResume,
                            params,
                        ),
                        &fixture.handler,
                    )
                    .await;
                json_rpc_success(frame, "json-rpc session/resume")
            }
            #[cfg(unix)]
            Self::UnixSidecar {
                transport,
                next_request_id,
                notifications,
                ..
            } => {
                let frame = request_over_ndjson_transport(
                    transport,
                    next_request_id,
                    notifications,
                    ClientRequestMethod::SessionResume,
                    params,
                    "unix sidecar session/resume",
                )
                .await;
                json_rpc_success(frame, "unix sidecar session/resume")
            }
        }
    }

    async fn session_close(&mut self, fixture: &Fixture, params: SessionCloseParams) {
        match self {
            Self::LocalTyped { client } => client
                .session_close(&fixture.handler, params)
                .await
                .expect("local session/close"),
            Self::JsonRpc {
                connection,
                next_request_id,
            } => {
                let frame = connection
                    .dispatch_client_request(
                        json_rpc_request(
                            next_request_id,
                            ClientRequestMethod::SessionClose,
                            params,
                        ),
                        &fixture.handler,
                    )
                    .await;
                json_rpc_success::<()>(frame, "json-rpc session/close");
            }
            #[cfg(unix)]
            Self::UnixSidecar {
                transport,
                next_request_id,
                notifications,
                ..
            } => {
                let frame = request_over_ndjson_transport(
                    transport,
                    next_request_id,
                    notifications,
                    ClientRequestMethod::SessionClose,
                    params,
                    "unix sidecar session/close",
                )
                .await;
                json_rpc_success::<()>(frame, "unix sidecar session/close");
            }
        }
    }

    async fn wait_for_session_result_stop_reason(
        &mut self,
        fixture: &Fixture,
        session_id: &coco_types::SessionId,
    ) -> Option<String> {
        match self {
            Self::LocalTyped { .. } | Self::JsonRpc { .. } => {
                wait_for_observed_outbound(fixture, |event| {
                    event.session_id == *session_id && event.kind == "session/result"
                })
                .await
                .session_result_stop_reason
            }
            #[cfg(unix)]
            Self::UnixSidecar {
                transport,
                notifications,
                ..
            } => loop {
                if let Some(stop_reason) = notifications.iter().find_map(|notification| {
                    notification_session_result_stop_reason(notification, session_id)
                }) {
                    break Some(stop_reason);
                }
                let frame = transport
                    .recv_frame()
                    .await
                    .expect("unix sidecar receive session/result")
                    .expect("unix sidecar frame before EOF");
                record_wire_frame_notifications(frame, notifications);
            },
        }
    }

    async fn shutdown(self) {
        match self {
            Self::LocalTyped { .. } | Self::JsonRpc { .. } => {}
            #[cfg(unix)]
            Self::UnixSidecar {
                mut shutdown_tx,
                listener_task,
                outbound_forwarder,
                ..
            } => {
                if let Some(shutdown_tx) = shutdown_tx.take() {
                    let _ = shutdown_tx.send(());
                }
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), listener_task).await;
                outbound_forwarder.abort();
                let _ = outbound_forwarder.await;
            }
        }
    }
}

fn json_rpc_request<T: serde::Serialize>(
    next_request_id: &mut i64,
    method: ClientRequestMethod,
    params: T,
) -> JsonRpcRequest {
    let id = *next_request_id;
    *next_request_id += 1;
    JsonRpcRequest::new(
        JsonRpcId::Number(id),
        method.as_str(),
        Some(serde_json::to_value(params).expect("serialize JSON-RPC params")),
    )
}

fn json_rpc_success<T: serde::de::DeserializeOwned>(frame: JsonRpcFrame, context: &str) -> T {
    match frame {
        JsonRpcFrame::Success(success) => {
            serde_json::from_value(success.result).expect("decode JSON-RPC success result")
        }
        JsonRpcFrame::Error(error) => panic!("{context} failed: {:?}", error.error),
        other => panic!("{context} returned unexpected frame: {other:?}"),
    }
}

fn append_durable_transcript_seed(
    fixture: &Fixture,
    session_id: &coco_types::SessionId,
    seed_text: &str,
) {
    let runtime = fixture
        .server
        .registry()
        .get(session_id)
        .expect("live runtime for transcript seed")
        .into_session();
    runtime
        .session_manager_handle()
        .store_for(runtime.original_cwd())
        .append_message(
            session_id.as_str(),
            &coco_session::storage::TranscriptEntry {
                entry_type: "user".to_string(),
                uuid: format!("{session_id}-resume-seed"),
                parent_uuid: None,
                logical_parent_uuid: None,
                session_id: Some(session_id.clone()),
                cwd: runtime.original_cwd().to_string_lossy().into_owned(),
                timestamp: "2026-07-13T00:00:00Z".to_string(),
                version: None,
                git_branch: None,
                is_sidechain: false,
                agent_id: None,
                message: Some(serde_json::json!({"role": "user", "content": seed_text})),
                usage: None,
                model: None,
                request_id: None,
                cost_usd: None,
                extra: serde_json::Map::new(),
            },
        )
        .expect("seed durable transcript");
}

#[cfg(unix)]
async fn request_over_ndjson_transport<T: serde::Serialize>(
    transport: &mut coco_app_server_transport::NdjsonUnixConnection,
    next_request_id: &mut i64,
    notifications: &mut Vec<JsonRpcNotification>,
    method: ClientRequestMethod,
    params: T,
    context: &str,
) -> JsonRpcFrame {
    let request = json_rpc_request(next_request_id, method, params);
    let expected_id = request.id.clone();
    transport
        .send_frame(&JsonRpcFrame::Request(request))
        .await
        .expect("send NDJSON transport request");
    recv_matching_ndjson_response(transport, expected_id, notifications, context).await
}

#[cfg(unix)]
async fn recv_matching_ndjson_response(
    transport: &mut coco_app_server_transport::NdjsonUnixConnection,
    expected_id: JsonRpcId,
    notifications: &mut Vec<JsonRpcNotification>,
    context: &str,
) -> JsonRpcFrame {
    loop {
        let frame = transport
            .recv_frame()
            .await
            .expect("receive NDJSON transport frame")
            .expect("NDJSON transport returned EOF");
        match frame {
            JsonRpcFrame::Success(success) if success.id == expected_id => {
                return JsonRpcFrame::Success(success);
            }
            JsonRpcFrame::Error(error) if error.id == expected_id => {
                return JsonRpcFrame::Error(error);
            }
            JsonRpcFrame::Notification(notification) => notifications.push(notification),
            other => panic!("{context} received unexpected frame: {other:?}"),
        }
    }
}

fn record_wire_frame_notifications(
    frame: JsonRpcFrame,
    notifications: &mut Vec<JsonRpcNotification>,
) {
    if let JsonRpcFrame::Notification(notification) = frame {
        notifications.push(notification);
    }
}

fn notification_session_result_stop_reason(
    notification: &JsonRpcNotification,
    session_id: &coco_types::SessionId,
) -> Option<String> {
    match notification.method.as_str() {
        "session/result" => {
            let params = notification.params.as_ref()?;
            (params.get("session_id")? == session_id.as_str())
                .then(|| params.get("stop_reason")?.as_str().map(str::to_string))?
        }
        "session/event" => {
            let envelope = notification.params.as_ref()?.get("envelope")?;
            (envelope.get("session_id")? == session_id.as_str()).then(|| {
                let event = envelope.get("event")?;
                (event.get("layer")?.as_str()? == "protocol").then_some(())?;
                let payload = event.get("payload")?;
                (payload.get("method")?.as_str()? == "session/result").then_some(())?;
                payload
                    .get("params")?
                    .get("stop_reason")?
                    .as_str()
                    .map(str::to_string)
            })?
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedOutbound {
    session_id: coco_types::SessionId,
    kind: &'static str,
    turn_id: Option<coco_types::TurnId>,
    session_result_total_turns: Option<i32>,
    session_result_input_tokens: Option<i64>,
    session_result_output_tokens: Option<i64>,
    session_result_stop_reason: Option<String>,
    turn_session_result_total_turns: Option<i32>,
    turn_session_result_stop_reason: Option<String>,
}

#[derive(Debug)]
struct TurnObservation {
    session_id: coco_types::SessionId,
    cwd: std::path::PathBuf,
    prompt: String,
    cancelled: bool,
}

struct TurnGate {
    started: tokio::sync::Barrier,
    release: tokio::sync::Semaphore,
    ignore_cancel: bool,
    result_emission: TurnResultEmission,
    drop_signal: Option<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnResultEmission {
    None,
    Standalone,
    Embedded,
    Both,
}

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

struct RecordingTurnRunner {
    observations: mpsc::UnboundedSender<TurnObservation>,
    gate: Option<Arc<TurnGate>>,
}

struct IsolatedTestBootstrapSource {
    runtime_config: Arc<coco_config::RuntimeConfig>,
    project_services: Arc<coco_app_runtime::ProjectServices>,
}

impl coco_app_runtime::BootstrapSource for IsolatedTestBootstrapSource {
    fn bootstrap_for_session(
        &self,
        cwd: &std::path::Path,
        _session_id_override: Option<&coco_types::SessionId>,
    ) -> Result<coco_app_runtime::SessionRuntimeBootstrapBuild, coco_app_runtime::BootstrapError>
    {
        let mut runtime_config = (*self.runtime_config).clone();
        runtime_config.settings.merged.fast_mode = Some(
            cwd.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.ends_with('a')),
        );
        Ok(coco_app_runtime::SessionRuntimeBootstrapBuild {
            bootstrap: Arc::new(SessionRuntimeBootstrap {
                runtime_config: Arc::new(runtime_config),
                tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
                model_id: "claude-opus-4-7".to_string(),
                system_prompt: "multi-session integration test".to_string(),
                permission_mode_availability: coco_types::PermissionModeAvailability::default(),
                permission_mode: coco_types::PermissionMode::default(),
                command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                    coco_commands::CommandRegistry::new(),
                ))),
                skill_manager: Arc::new(coco_skills::SkillManager::new()),
                project_services: Arc::clone(&self.project_services),
                agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
            }),
            config_reloader: None,
        })
    }
}

fn spawn_client_mcp_responder(
    mut requests: mpsc::Receiver<coco_types::ServerRequestDelivery>,
    server: Arc<AppServer<AppSessionHandle>>,
    expected_server: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(delivery) = requests.recv().await {
            let coco_types::ServerRequest::McpRouteMessage(params) = delivery.request else {
                continue;
            };
            assert_eq!(params.server_name, expected_server);
            let id = params
                .message
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let method = params
                .message
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": expected_server, "version": "1.0.0"}
                }),
                "tools/list" => serde_json::json!({
                    "tools": [{
                        "name": "isolated_tool",
                        "description": format!("tool from {expected_server}"),
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }),
                _ => serde_json::json!({}),
            };
            server
                .resolve_server_request(
                    &InteractiveTarget {
                        session_id: delivery.session_id,
                        surface_id: delivery.surface_id,
                    },
                    coco_app_server::ServerRequestReply::McpRouteMessage {
                        request_id: delivery.request_id.as_display(),
                        result: serde_json::json!({
                            "message": {"jsonrpc": "2.0", "id": id, "result": result}
                        }),
                    },
                )
                .expect("resolve client MCP route request");
        }
    })
}

impl TurnRunner for RecordingTurnRunner {
    fn run_turn<'a>(
        &'a self,
        session: coco_agent_host::session_runtime::SessionHandle,
        _app_server: Arc<AppServer<AppSessionHandle>>,
        params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        event_tx: mpsc::Sender<CoreEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let observations = self.observations.clone();
        let gate = self.gate.clone();
        Box::pin(async move {
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnStarted(
                    coco_types::TurnStartedParams {
                        turn_id: turn_id.clone(),
                    },
                )))
                .await?;
            let result_emission = gate
                .as_ref()
                .map_or(TurnResultEmission::None, |gate| gate.result_emission);
            let cancelled = if let Some(gate) = gate {
                let _drop_signal = if let Some(sender) = &gate.drop_signal {
                    Some(DropSignal(sender.lock().await.take()))
                } else {
                    None
                };
                gate.started.wait().await;
                if gate.ignore_cancel {
                    if let Ok(permit) = gate.release.acquire().await {
                        permit.forget();
                    }
                    cancel.is_cancelled()
                } else {
                    tokio::select! {
                        () = cancel.cancelled() => true,
                        permit = gate.release.acquire() => {
                            if let Ok(permit) = permit {
                                permit.forget();
                            }
                            cancel.is_cancelled()
                        },
                    }
                }
            } else {
                cancel.is_cancelled()
            };
            observations
                .send(TurnObservation {
                    session_id: session.session_id().clone(),
                    cwd: session.original_cwd().clone(),
                    prompt: params.prompt,
                    cancelled,
                })
                .map_err(|_| anyhow::anyhow!("turn observation receiver dropped"))?;
            let turn_result = (result_emission != TurnResultEmission::None).then(|| {
                coco_types::SessionResultParams {
                    session_id: session.session_id().clone(),
                    total_turns: 1,
                    duration_ms: 77,
                    duration_api_ms: 55,
                    is_error: false,
                    stop_reason: "interrupted".to_string(),
                    total_cost_usd: 0.125,
                    usage: coco_types::TokenUsage {
                        input_tokens: coco_types::InputTokens {
                            total: 7,
                            no_cache: 7,
                            cache_read: 0,
                            cache_write: 0,
                        },
                        output_tokens: coco_types::OutputTokens {
                            total: 11,
                            text: 11,
                            reasoning: 0,
                        },
                    },
                    model_usage: std::collections::HashMap::new(),
                    permission_denials: Vec::new(),
                    result: Some("interrupted result".to_string()),
                    errors: Vec::new(),
                    structured_output: None,
                    fast_mode_state: None,
                    num_api_calls: Some(1),
                }
            });
            if matches!(
                result_emission,
                TurnResultEmission::Standalone | TurnResultEmission::Both
            ) && let Some(result) = turn_result.clone()
            {
                event_tx
                    .send(CoreEvent::Protocol(ServerNotification::SessionResult(
                        Box::new(result),
                    )))
                    .await?;
            }
            let mut ended = if cancelled {
                coco_types::TurnEndedParams::interrupted(
                    turn_id,
                    None,
                    coco_types::TurnAbortReason::UserCancel,
                )
            } else {
                coco_types::TurnEndedParams::completed(
                    turn_id,
                    Some(coco_types::TokenUsage::default()),
                    Some(coco_messages::StopReason::EndTurn),
                )
            };
            if matches!(
                result_emission,
                TurnResultEmission::Embedded | TurnResultEmission::Both
            ) && let Some(result) = turn_result
            {
                ended = ended.with_session_result(result);
            }
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnEnded(ended)))
                .await?;
            Ok(())
        })
    }
}

fn spawn_outbound_collector(
    mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    observed: Arc<tokio::sync::Mutex<Vec<ObservedOutbound>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(outbound) = outbound_rx.recv().await {
            if let OutboundMessage::SessionEvent {
                session_id, event, ..
            } = outbound
            {
                observed.lock().await.push(ObservedOutbound {
                    session_id,
                    kind: core_event_kind(event.as_ref()),
                    turn_id: core_event_turn_id(event.as_ref()),
                    session_result_total_turns: session_result_total_turns(event.as_ref()),
                    session_result_input_tokens: session_result_input_tokens(event.as_ref()),
                    session_result_output_tokens: session_result_output_tokens(event.as_ref()),
                    session_result_stop_reason: session_result_stop_reason(event.as_ref()),
                    turn_session_result_total_turns: turn_session_result_total_turns(
                        event.as_ref(),
                    ),
                    turn_session_result_stop_reason: turn_session_result_stop_reason(
                        event.as_ref(),
                    ),
                });
            }
        }
    })
}

fn core_event_kind(event: &CoreEvent) -> &'static str {
    match event {
        CoreEvent::Protocol(ServerNotification::SessionResult(_)) => "session/result",
        CoreEvent::Protocol(ServerNotification::TurnStarted(_)) => "turn/started",
        CoreEvent::Protocol(ServerNotification::TurnEnded(_)) => "turn/ended",
        CoreEvent::Protocol(_) => "protocol",
        CoreEvent::Stream(_) => "stream",
        CoreEvent::Tui(_) => "tui",
    }
}

fn core_event_turn_id(event: &CoreEvent) -> Option<coco_types::TurnId> {
    match event {
        CoreEvent::Protocol(ServerNotification::TurnStarted(params)) => {
            Some(params.turn_id.clone())
        }
        CoreEvent::Protocol(ServerNotification::TurnEnded(params)) => Some(params.turn_id.clone()),
        _ => None,
    }
}

fn session_result_total_turns(event: &CoreEvent) -> Option<i32> {
    match event {
        CoreEvent::Protocol(ServerNotification::SessionResult(params)) => Some(params.total_turns),
        _ => None,
    }
}

fn session_result_input_tokens(event: &CoreEvent) -> Option<i64> {
    match event {
        CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
            Some(params.usage.input_tokens.total)
        }
        _ => None,
    }
}

fn session_result_output_tokens(event: &CoreEvent) -> Option<i64> {
    match event {
        CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
            Some(params.usage.output_tokens.total)
        }
        _ => None,
    }
}

fn session_result_stop_reason(event: &CoreEvent) -> Option<String> {
    match event {
        CoreEvent::Protocol(ServerNotification::SessionResult(params)) => {
            Some(params.stop_reason.clone())
        }
        _ => None,
    }
}

fn turn_session_result_total_turns(event: &CoreEvent) -> Option<i32> {
    match event {
        CoreEvent::Protocol(ServerNotification::TurnEnded(params)) => params
            .session_result
            .as_ref()
            .map(|result| result.total_turns),
        _ => None,
    }
}

fn turn_session_result_stop_reason(event: &CoreEvent) -> Option<String> {
    match event {
        CoreEvent::Protocol(ServerNotification::TurnEnded(params)) => params
            .session_result
            .as_ref()
            .map(|result| result.stop_reason.clone()),
        _ => None,
    }
}

async fn wait_for_observed_outbound(
    fixture: &Fixture,
    matches: impl Fn(&ObservedOutbound) -> bool,
) -> ObservedOutbound {
    loop {
        if let Some(event) = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .find(|event| matches(event))
            .cloned()
        {
            return event;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[allow(clippy::expect_used)]
async fn fixture_with_turn_gate(turn_gate: Option<Arc<TurnGate>>) -> Fixture {
    fixture_with_turn_gate_and_timeout(
        turn_gate,
        coco_agent_host::app_server_host::APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
}

#[allow(clippy::expect_used)]
async fn fixture_with_turn_gate_and_timeout(
    turn_gate: Option<Arc<TurnGate>>,
    turn_drain_timeout: std::time::Duration,
) -> Fixture {
    let home = tempfile::TempDir::new().expect("fixture home");
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings {
            file_checkpointing_enabled: true,
            models: coco_config::ModelSelectionSettings {
                main: Some(coco_config::RoleSlots::new(
                    coco_types::ProviderModelSelection {
                        provider: "anthropic".to_string(),
                        model_id: "claude-opus-4-7".to_string(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = coco_config::build_runtime_config_with(
        settings,
        coco_config::EnvSnapshot::default(),
        coco_config::RuntimeOverrides::default(),
        coco_config::CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");
    let process_runtime = coco_app_runtime::ProcessRuntime::global();
    let project_services = Arc::new(coco_app_runtime::ProjectServices::load(
        home.path(),
        home.path(),
    ));
    let runtime_config = Arc::new(runtime_config);
    let runtime_factory = SessionRuntimeFactory::new(SessionRuntimeFactoryOpts {
        cli: Arc::new(AgentHostOptions::default()),
        bootstrap_source: SessionRuntimeBootstrapSource::from_source(Arc::new(
            IsolatedTestBootstrapSource {
                runtime_config,
                project_services,
            },
        )),
        cwd: home.path().to_path_buf(),
        model_runtimes: None,
        session_manager: Arc::new(coco_session::SessionManager::new(
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        process_runtime: Arc::clone(&process_runtime),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::noninteractive(),
        is_non_interactive: false,
    });
    let state = Arc::new(AppServerHostState::new(HostInputs {
        startup_cwd: Some(home.path().to_path_buf()),
        runtime_replacement: Some(RuntimeReplacementContext {
            runtime_factory,
            process_runtime,
            cwd: home.path().to_path_buf(),
            requires_structured_output: false,
            integration_options:
                coco_agent_host::session_bootstrap::SessionIntegrationOptions::default(),
        }),
        ..Default::default()
    }));
    let server = Arc::new(AppServer::new(8, 16));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 16);
    let (notif_tx, notif_rx) = mpsc::channel(16);
    let observed_outbound = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let outbound_collector = spawn_outbound_collector(notif_rx, Arc::clone(&observed_outbound));
    let handler = AppServerHostHandler::with_local_app_server_and_turn_drain_timeout(
        Arc::clone(&state),
        notif_tx,
        Arc::clone(&server),
        turn_drain_timeout,
    );
    let (turn_tx, turn_rx) = mpsc::unbounded_channel();
    handler
        .set_turn_runner(Arc::new(RecordingTurnRunner {
            observations: turn_tx,
            gate: turn_gate.clone(),
        }))
        .await;
    Fixture {
        _home: home,
        state,
        server,
        adapter,
        handler,
        turn_observations: tokio::sync::Mutex::new(turn_rx),
        turn_gate,
        observed_outbound,
        _outbound_collector: outbound_collector,
    }
}

#[tokio::test]
async fn shortcut_turns_emit_terminal_session_result_and_accounting() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start shortcut session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.clone().expect("interactive surface"),
        };

        let cost_prompt = coco_commands::handlers::cost::handler(String::new())
            .await
            .expect("cost sentinel");
        let cost_turn = client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: cost_prompt,
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start cost shortcut");
        let cost_ended = wait_for_observed_outbound(&fixture, |event| {
            event.session_id == started.session_id
                && event.kind == "turn/ended"
                && event.turn_id.as_ref() == Some(&cost_turn.turn_id)
        })
        .await;
        assert_eq!(cost_ended.turn_session_result_total_turns, Some(1));
        assert_eq!(
            cost_ended.turn_session_result_stop_reason.as_deref(),
            Some("shortcut_completed")
        );

        let compact_prompt = coco_commands::handlers::compact::handler(String::new())
            .await
            .expect("compact sentinel");
        let compact_turn = client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: compact_prompt,
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start compact shortcut");
        let compact_ended = wait_for_observed_outbound(&fixture, |event| {
            event.session_id == started.session_id
                && event.kind == "turn/ended"
                && event.turn_id.as_ref() == Some(&compact_turn.turn_id)
        })
        .await;
        assert_eq!(compact_ended.turn_session_result_total_turns, Some(1));
        assert_eq!(
            compact_ended.turn_session_result_stop_reason.as_deref(),
            Some("manual_compact_skipped")
        );

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect("close shortcut session");
        let final_result = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .rev()
            .find(|event| event.session_id == started.session_id && event.kind == "session/result")
            .cloned()
            .expect("final session/result event");
        assert_eq!(final_result.session_result_total_turns, Some(2));
        assert_eq!(
            final_result.session_result_stop_reason.as_deref(),
            Some("manual_compact_skipped")
        );
    })
    .await
    .expect("shortcut terminal session_result regression timed out");
}

#[tokio::test]
async fn one_connection_holds_two_independent_interactive_authorities() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let first = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start A");
        let second = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start B");

        assert_ne!(first.session_id, second.session_id);
        assert_ne!(first.surface_id, second.surface_id);
        let live = fixture.server.list_live_sessions();
        assert_eq!(live.len(), 2);
        assert!(
            live.iter()
                .all(|summary| summary.surface_counts.attached == 1)
        );
    })
    .await
    .expect("interactive authority isolation timed out");
}

#[tokio::test]
async fn session_start_initial_messages_seed_lifecycle_owned_history() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(
                &fixture.handler,
                SessionStartParams {
                    initial_messages: vec![coco_messages::create_user_message(
                        "seeded-before-start",
                    )],
                    ..Default::default()
                },
            )
            .await
            .expect("start with initial messages");

        let read = client
            .session_read(
                &fixture.handler,
                SessionReadParams {
                    target: coco_types::SessionTarget {
                        session_id: started.session_id,
                    },
                    cursor: None,
                    limit: None,
                },
            )
            .await
            .expect("read started session");

        assert!(
            read.messages
                .iter()
                .any(|message| message.to_string().contains("seeded-before-start"))
        );
    })
    .await
    .expect("initial messages lifecycle seed timed out");
}

#[tokio::test]
async fn lifecycle_conformance_surfaces_share_start_read_close_contract() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut surfaces = vec![
            LifecycleConformanceSurface::LocalTyped,
            LifecycleConformanceSurface::JsonRpc,
        ];
        #[cfg(unix)]
        surfaces.push(LifecycleConformanceSurface::UnixSidecar);

        for surface in surfaces {
            lifecycle_conformance_start_read_close(surface).await;
        }
    })
    .await
    .expect("lifecycle conformance timed out");
}

#[tokio::test]
async fn lifecycle_conformance_surfaces_share_resume_contract() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut surfaces = vec![
            LifecycleConformanceSurface::LocalTyped,
            LifecycleConformanceSurface::JsonRpc,
        ];
        #[cfg(unix)]
        surfaces.push(LifecycleConformanceSurface::UnixSidecar);

        for surface in surfaces {
            lifecycle_conformance_resume(surface).await;
        }
    })
    .await
    .expect("lifecycle resume conformance timed out");
}

async fn lifecycle_conformance_start_read_close(surface: LifecycleConformanceSurface) {
    let fixture = fixture().await;
    let mut client = surface.connect(&fixture).await;
    let seed_text = format!("seeded-through-{}", surface.label());

    let started = client
        .session_start(
            &fixture,
            SessionStartParams {
                initial_messages: vec![coco_messages::create_user_message(&seed_text)],
                ..Default::default()
            },
        )
        .await;
    let surface_id = started
        .surface_id
        .clone()
        .expect("session/start must return an interactive surface id");

    let live = fixture.server.list_live_sessions();
    assert_eq!(live.len(), 1, "{} live session count", surface.label());
    assert_eq!(live[0].session_id, started.session_id);
    assert_eq!(live[0].surface_counts.attached, 1);

    let read = client
        .session_read(
            &fixture,
            SessionReadParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await;
    assert_eq!(read.session.session_id, started.session_id);
    assert!(
        read.messages
            .iter()
            .any(|message| message.to_string().contains(&seed_text)),
        "{} session/read must expose lifecycle-seeded history",
        surface.label()
    );

    client
        .session_close(
            &fixture,
            SessionCloseParams {
                target: SessionCloseTarget::Interactive {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id,
                    },
                },
            },
        )
        .await;

    assert!(
        fixture.server.list_live_sessions().is_empty(),
        "{} session/close must remove the live registry slot",
        surface.label()
    );
    assert_eq!(
        client
            .wait_for_session_result_stop_reason(&fixture, &started.session_id)
            .await
            .as_deref(),
        Some("closed"),
        "{} close result reason",
        surface.label()
    );
    client.shutdown().await;
}

async fn lifecycle_conformance_resume(surface: LifecycleConformanceSurface) {
    let fixture = fixture().await;
    let mut client = surface.connect(&fixture).await;
    let seed_text = format!("resumed-through-{}", surface.label());

    let started = client
        .session_start(&fixture, SessionStartParams::default())
        .await;
    let surface_id = started
        .surface_id
        .clone()
        .expect("session/start must return an interactive surface id");
    append_durable_transcript_seed(&fixture, &started.session_id, &seed_text);

    client
        .session_close(
            &fixture,
            SessionCloseParams {
                target: SessionCloseTarget::Interactive {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id,
                    },
                },
            },
        )
        .await;
    assert!(
        fixture.server.list_live_sessions().is_empty(),
        "{} pre-resume close must remove the live registry slot",
        surface.label()
    );

    let resumed = client
        .session_resume(
            &fixture,
            SessionResumeParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
            },
        )
        .await;
    assert_eq!(resumed.session.session_id, started.session_id);
    let resumed_surface_id = resumed
        .surface_id
        .clone()
        .expect("session/resume must return an interactive surface id");
    let live = fixture.server.list_live_sessions();
    assert_eq!(
        live.len(),
        1,
        "{} resumed live session count",
        surface.label()
    );
    assert_eq!(live[0].session_id, started.session_id);
    assert_eq!(live[0].surface_counts.attached, 1);

    let read = client
        .session_read(
            &fixture,
            SessionReadParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await;
    assert_eq!(read.session.session_id, started.session_id);
    assert!(
        read.messages
            .iter()
            .any(|message| message.to_string().contains(&seed_text)),
        "{} session/read must expose resumed durable history",
        surface.label()
    );

    client
        .session_close(
            &fixture,
            SessionCloseParams {
                target: SessionCloseTarget::Interactive {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id: resumed_surface_id,
                    },
                },
            },
        )
        .await;
    client.shutdown().await;
}

#[tokio::test]
async fn one_connection_runs_and_interrupts_two_runtime_backed_turns_independently() {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        one_connection_runtime_isolation_scenario(),
    )
    .await
    .expect("one-connection runtime isolation timed out");
}

async fn one_connection_runtime_isolation_scenario() {
    let gate = Arc::new(TurnGate {
        started: tokio::sync::Barrier::new(3),
        release: tokio::sync::Semaphore::new(0),
        ignore_cancel: false,
        result_emission: TurnResultEmission::None,
        drop_signal: None,
    });
    let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
    let client = LocalServerClient::connect_local(&fixture.adapter);
    let cwd_a = fixture._home.path().join("project-a");
    let cwd_b = fixture._home.path().join("project-b");
    std::fs::create_dir_all(&cwd_a).expect("create project A");
    std::fs::create_dir_all(&cwd_b).expect("create project B");
    let first = client
        .session_start(
            &fixture.handler,
            SessionStartParams {
                cwd: Some(cwd_a.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("start A");
    let second = client
        .session_start(
            &fixture.handler,
            SessionStartParams {
                cwd: Some(cwd_b.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("start B");
    let target_a = InteractiveTarget {
        session_id: first.session_id.clone(),
        surface_id: first.surface_id.clone().expect("A surface"),
    };
    let target_b = InteractiveTarget {
        session_id: second.session_id.clone(),
        surface_id: second.surface_id.clone().expect("B surface"),
    };

    for (target, prompt) in [
        (target_a.clone(), "prompt-a"),
        (target_b.clone(), "prompt-b"),
    ] {
        client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target,
                    prompt: prompt.to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start targeted turn");
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.started.wait())
        .await
        .expect("both turns reach the concurrency barrier");

    client
        .turn_interrupt(&fixture.handler, target_a)
        .await
        .expect("interrupt A only");
    fixture
        .turn_gate
        .as_ref()
        .expect("turn gate")
        .release
        .add_permits(2);

    let mut observations = Vec::new();
    let mut receiver = fixture.turn_observations.lock().await;
    for _ in 0..2 {
        observations.push(
            tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
                .await
                .expect("turn observation timeout")
                .expect("turn observation"),
        );
    }
    observations.sort_by(|left, right| left.prompt.cmp(&right.prompt));

    assert_eq!(observations[0].session_id, first.session_id);
    assert_eq!(observations[0].cwd, cwd_a);
    assert_eq!(observations[0].prompt, "prompt-a");
    assert!(observations[0].cancelled);
    assert_eq!(observations[1].session_id, second.session_id);
    assert_eq!(observations[1].cwd, cwd_b);
    assert_eq!(observations[1].prompt, "prompt-b");
    assert!(!observations[1].cancelled);

    let runtime_a = fixture
        .server
        .registry()
        .get(&first.session_id)
        .expect("A runtime")
        .into_session();
    let runtime_b = fixture
        .server
        .registry()
        .get(&second.session_id)
        .expect("B runtime")
        .into_session();
    let target_a = InteractiveTarget {
        session_id: first.session_id.clone(),
        surface_id: first.surface_id.expect("A surface"),
    };
    let target_b = InteractiveTarget {
        session_id: second.session_id.clone(),
        surface_id: second.surface_id.expect("B surface"),
    };
    client
        .set_model(
            &fixture.handler,
            coco_types::SetModelParams {
                target: target_a.clone(),
                model: Some("model-a".to_string()),
            },
        )
        .await
        .expect("set A model");
    client
        .set_model(
            &fixture.handler,
            coco_types::SetModelParams {
                target: target_b.clone(),
                model: Some("model-b".to_string()),
            },
        )
        .await
        .expect("set B model");
    client
        .set_permission_mode(
            &fixture.handler,
            coco_types::SetPermissionModeParams {
                target: target_a.clone(),
                mode: coco_types::PermissionMode::Plan,
            },
        )
        .await
        .expect("set A permission mode");
    client
        .set_permission_mode(
            &fixture.handler,
            coco_types::SetPermissionModeParams {
                target: target_b.clone(),
                mode: coco_types::PermissionMode::DontAsk,
            },
        )
        .await
        .expect("set B permission mode");
    client
        .set_thinking(
            &fixture.handler,
            coco_types::SetThinkingParams {
                target: target_a.clone(),
                thinking_level: Some(coco_types::ThinkingLevel {
                    effort: coco_types::ReasoningEffort::High,
                    budget_tokens: None,
                    options: std::collections::HashMap::new(),
                }),
            },
        )
        .await
        .expect("set A thinking");
    client
        .set_agent_color(
            &fixture.handler,
            coco_types::SetAgentColorParams {
                target: target_b.clone(),
                color: Some(coco_types::AgentColorName::Cyan),
            },
        )
        .await
        .expect("set B color");
    client
        .update_env(
            &fixture.handler,
            coco_types::UpdateEnvParams {
                target: target_a.clone(),
                env: std::collections::HashMap::from([(
                    "SESSION_MARKER".to_string(),
                    "A".to_string(),
                )]),
            },
        )
        .await
        .expect("update A env");
    client
        .update_env(
            &fixture.handler,
            coco_types::UpdateEnvParams {
                target: target_b.clone(),
                env: std::collections::HashMap::from([(
                    "SESSION_MARKER".to_string(),
                    "B".to_string(),
                )]),
            },
        )
        .await
        .expect("update B env");

    let config_a = runtime_a.current_engine_config().await;
    let config_b = runtime_b.current_engine_config().await;
    assert_eq!(config_a.model_id, "model-a");
    assert_eq!(config_b.model_id, "model-b");
    assert_eq!(config_a.permission_mode, coco_types::PermissionMode::Plan);
    assert_eq!(
        config_b.permission_mode,
        coco_types::PermissionMode::DontAsk
    );
    assert_eq!(
        config_a.thinking_level.as_ref().map(|level| level.effort),
        Some(coco_types::ReasoningEffort::High)
    );
    assert_eq!(
        runtime_b.app_state().read().await.agent_color,
        Some(coco_types::AgentColorName::Cyan)
    );
    assert_eq!(
        runtime_a.session_env_snapshot().expect("A env")["SESSION_MARKER"],
        "A"
    );
    assert_eq!(
        runtime_b.session_env_snapshot().expect("B env")["SESSION_MARKER"],
        "B"
    );

    runtime_a
        .history()
        .lock()
        .await
        .push(coco_messages::create_user_message("history-a"));
    runtime_b
        .history()
        .lock()
        .await
        .push(coco_messages::create_user_message("history-b"));
    let read_a = client
        .session_read(
            &fixture.handler,
            coco_types::SessionReadParams {
                target: coco_types::SessionTarget {
                    session_id: first.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("read A");
    let read_b = client
        .session_read(
            &fixture.handler,
            coco_types::SessionReadParams {
                target: coco_types::SessionTarget {
                    session_id: second.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("read B");
    assert!(
        read_a
            .messages
            .iter()
            .any(|message| message.to_string().contains("history-a"))
    );
    assert!(
        !read_a
            .messages
            .iter()
            .any(|message| message.to_string().contains("history-b"))
    );
    assert!(
        read_b
            .messages
            .iter()
            .any(|message| message.to_string().contains("history-b"))
    );
    let turns_a = client
        .session_turns_list(
            &fixture.handler,
            coco_types::SessionTurnsListParams {
                target: coco_types::SessionTarget {
                    session_id: first.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("list A turns");
    let turns_b = client
        .session_turns_list(
            &fixture.handler,
            coco_types::SessionTurnsListParams {
                target: coco_types::SessionTarget {
                    session_id: second.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("list B turns");
    assert_eq!(turns_a.session.session_id, first.session_id);
    assert_eq!(turns_b.session.session_id, second.session_id);
    assert_eq!(turns_a.turns.len(), 1);
    assert_eq!(turns_b.turns.len(), 1);

    for (runtime, target, path, snapshot, original, changed) in [
        (
            &runtime_a,
            target_a,
            cwd_a.join("rewind-a.txt"),
            "snapshot-a",
            "original-a",
            "changed-a",
        ),
        (
            &runtime_b,
            target_b,
            cwd_b.join("rewind-b.txt"),
            "snapshot-b",
            "original-b",
            "changed-b",
        ),
    ] {
        std::fs::write(&path, original).expect("write original rewind file");
        let history = runtime.file_history().expect("file history enabled");
        {
            let mut history = history.write().await;
            history.track_file(path.clone());
            history
                .make_snapshot(
                    snapshot,
                    runtime.config_home(),
                    runtime.session_id().as_str(),
                )
                .await
                .expect("make rewind snapshot");
        }
        std::fs::write(&path, changed).expect("write changed rewind file");
        let rewound = client
            .rewind_files(
                &fixture.handler,
                coco_types::RewindFilesParams {
                    target,
                    user_message_id: snapshot.to_string(),
                    dry_run: false,
                },
            )
            .await
            .expect("rewind targeted file");
        assert!(!rewound.dry_run);
        assert_eq!(
            std::fs::read_to_string(path).expect("read rewound file"),
            original
        );
    }
}

#[tokio::test]
async fn two_initialized_connections_keep_profiles_runtimes_and_writers_isolated() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let fixture = fixture().await;
        let mut client_a = LocalServerClient::connect_local(&fixture.adapter);
        let mut client_b = LocalServerClient::connect_local(&fixture.adapter);
        let responder_a = spawn_client_mcp_responder(
            client_a.take_server_requests(),
            Arc::clone(&fixture.server),
            "mcp-a",
        );
        let responder_b = spawn_client_mcp_responder(
            client_b.take_server_requests(),
            Arc::clone(&fixture.server),
            "mcp-b",
        );
        let handler_a = fixture
            .handler
            .open(coco_app_server::ConnectionKey::generate());
        let handler_b = fixture
            .handler
            .open(coco_app_server::ConnectionKey::generate());
        let initialize = |profile: &str, tool: &str| InitializeParams {
            system_prompt: Some(profile.to_string()),
            client_mcp_servers: Some(vec![profile.replace("profile", "mcp")]),
            agents: Some(std::collections::HashMap::from([(
                format!("agent-{tool}"),
                coco_types::ClientAgentDefinition {
                    tools: Some(vec![tool.to_string()]),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };
        client_a
            .initialize(handler_a.as_ref(), initialize("profile-a", "Read"))
            .await
            .expect("initialize A");
        client_b
            .initialize(handler_b.as_ref(), initialize("profile-b", "Bash"))
            .await
            .expect("initialize B");
        let cwd_a = fixture._home.path().join("initialized-a");
        let cwd_b = fixture._home.path().join("initialized-b");
        std::fs::create_dir_all(&cwd_a).expect("create A cwd");
        std::fs::create_dir_all(&cwd_b).expect("create B cwd");
        let first = client_a
            .session_start(
                handler_a.as_ref(),
                SessionStartParams {
                    cwd: Some(cwd_a.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .await
            .expect("start A");
        let second = client_b
            .session_start(
                handler_b.as_ref(),
                SessionStartParams {
                    cwd: Some(cwd_b.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .await
            .expect("start B");
        let make_turn =
            |started: &coco_types::SessionStartResult, prompt: &str| coco_types::TurnStartParams {
                target: InteractiveTarget {
                    session_id: started.session_id.clone(),
                    surface_id: started.surface_id.clone().expect("surface"),
                },
                prompt: prompt.to_string(),
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
            };
        let (turn_a, turn_b) = tokio::join!(
            client_a.turn_start(&fixture.handler, make_turn(&first, "profile-a")),
            client_b.turn_start(&fixture.handler, make_turn(&second, "profile-b")),
        );
        assert_ne!(
            turn_a.expect("turn A").turn_id,
            turn_b.expect("turn B").turn_id
        );
        let runtime_a = fixture
            .server
            .registry()
            .get(&first.session_id)
            .expect("A runtime")
            .into_session();
        let runtime_b = fixture
            .server
            .registry()
            .get(&second.session_id)
            .expect("B runtime")
            .into_session();
        assert!(
            runtime_a
                .callback_requirements()
                .hook_callback_ids
                .is_empty()
        );
        assert!(
            runtime_b
                .callback_requirements()
                .hook_callback_ids
                .is_empty()
        );
        assert_eq!(runtime_a.original_cwd(), &cwd_a);
        assert_eq!(runtime_b.original_cwd(), &cwd_b);
        assert_eq!(
            runtime_a.runtime_config().settings.merged.fast_mode,
            Some(true)
        );
        assert_eq!(
            runtime_b.runtime_config().settings.merged.fast_mode,
            Some(false)
        );
        runtime_a
            .history()
            .lock()
            .await
            .push(coco_messages::create_user_message("profile-history-a"));
        runtime_b
            .history()
            .lock()
            .await
            .push(coco_messages::create_user_message("profile-history-b"));
        assert!(
            runtime_a
                .history()
                .lock()
                .await
                .snapshot()
                .iter()
                .any(|message| {
                    coco_messages::wrapping::extract_text_from_message(message)
                        .contains("profile-history-a")
                })
        );
        assert!(
            !runtime_a
                .history()
                .lock()
                .await
                .snapshot()
                .iter()
                .any(|message| {
                    coco_messages::wrapping::extract_text_from_message(message)
                        .contains("profile-history-b")
                })
        );
        assert!(
            runtime_a
                .tools()
                .get_by_name("mcp__mcp-a__isolated_tool")
                .is_some()
        );
        assert!(
            runtime_b
                .tools()
                .get_by_name("mcp__mcp-b__isolated_tool")
                .is_some()
        );
        assert!(
            runtime_a
                .tools()
                .get_by_name("mcp__mcp-b__isolated_tool")
                .is_none()
        );
        assert!(
            runtime_b
                .tools()
                .get_by_name("mcp__mcp-a__isolated_tool")
                .is_none()
        );
        let mut observations = fixture.turn_observations.lock().await;
        for _ in 0..2 {
            observations
                .recv()
                .await
                .expect("initialized turn completes");
        }
        drop(observations);
        client_a.disconnect();
        client_b.disconnect();
        let closer = LocalServerClient::connect_local(&fixture.adapter);
        for session_id in [first.session_id, second.session_id] {
            closer
                .session_close(
                    &fixture.handler,
                    SessionCloseParams {
                        target: SessionCloseTarget::Orphaned {
                            target: coco_types::SessionTarget { session_id },
                        },
                    },
                )
                .await
                .expect("close initialized session");
        }
        responder_a.await.expect("MCP responder A exits");
        responder_b.await.expect("MCP responder B exits");
    })
    .await
    .expect("initialized connection isolation timed out");
}

#[tokio::test]
async fn cross_connection_surface_authority_is_rejected_without_mutation() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cross_connection_surface_authority_scenario(),
    )
    .await
    .expect("cross-connection authority scenario timed out");
}

async fn cross_connection_surface_authority_scenario() {
    let fixture = fixture().await;
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let attacker = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start owner session");
    let target = InteractiveTarget {
        session_id: started.session_id.clone(),
        surface_id: started.surface_id.expect("interactive surface"),
    };

    let error = attacker
        .turn_interrupt(&fixture.handler, target)
        .await
        .expect_err("foreign connection cannot use surface");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_wrong_connection"))
    );
    assert_eq!(fixture.server.list_live_sessions().len(), 1);
}

#[tokio::test]
async fn mismatched_session_surface_pair_is_rejected_with_stable_kind() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        mismatched_session_surface_scenario(),
    )
    .await
    .expect("mismatched session/surface scenario timed out");
}

async fn mismatched_session_surface_scenario() {
    let fixture = fixture().await;
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let first = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");

    let error = owner
        .turn_interrupt(
            &fixture.handler,
            InteractiveTarget {
                session_id: second.session_id,
                surface_id: first.surface_id.expect("A surface"),
            },
        )
        .await
        .expect_err("surface and session must be correlated");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_wrong_session"))
    );
}

#[tokio::test]
async fn passive_surface_cannot_issue_interactive_mutation() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        passive_surface_mutation_scenario(),
    )
    .await
    .expect("passive mutation scenario timed out");
}

async fn passive_surface_mutation_scenario() {
    let fixture = fixture().await;
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start session");
    let observer = LocalServerClient::connect_local(&fixture.adapter);
    let passive = observer
        .attach_passive_session(started.session_id.clone())
        .expect("attach passive surface");

    let error = observer
        .turn_interrupt(
            &fixture.handler,
            InteractiveTarget {
                session_id: passive.session_id().clone(),
                surface_id: passive.surface_id().clone(),
            },
        )
        .await
        .expect_err("passive surface has no mutation authority");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_not_interactive"))
    );
}

#[tokio::test]
async fn targeted_project_and_local_config_writes_cannot_modify_the_sibling_project() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        targeted_config_write_scenario(),
    )
    .await
    .expect("targeted config-write scenario timed out");
}

async fn targeted_config_write_scenario() {
    let fixture = fixture().await;
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let cwd_a = fixture._home.path().join("config-project-a");
    let cwd_b = fixture._home.path().join("config-project-b");
    std::fs::create_dir_all(&cwd_a).expect("create project A");
    std::fs::create_dir_all(&cwd_b).expect("create project B");
    let first = owner
        .session_start(
            &fixture.handler,
            SessionStartParams {
                cwd: Some(cwd_a.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("start A");
    let second = owner
        .session_start(
            &fixture.handler,
            SessionStartParams {
                cwd: Some(cwd_b.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("start B");

    for (started, value) in [(first, true), (second, false)] {
        let target = InteractiveTarget {
            session_id: started.session_id,
            surface_id: started.surface_id.expect("interactive surface"),
        };
        owner
            .config_write(
                &fixture.handler,
                ConfigWriteParams {
                    target: ConfigWriteTarget::Project(target.clone()),
                    key: "fast_mode".to_string(),
                    value: serde_json::json!(value),
                },
            )
            .await
            .expect("write targeted project config");
        owner
            .config_write(
                &fixture.handler,
                ConfigWriteParams {
                    target: ConfigWriteTarget::Local(target),
                    key: "fast_mode".to_string(),
                    value: serde_json::json!(!value),
                },
            )
            .await
            .expect("write targeted local config");
    }

    let settings_a =
        std::fs::read_to_string(coco_config::global_config::project_settings_path(&cwd_a))
            .expect("read project A settings");
    let settings_b =
        std::fs::read_to_string(coco_config::global_config::project_settings_path(&cwd_b))
            .expect("read project B settings");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&settings_a).expect("parse A")["fast_mode"],
        serde_json::json!(true)
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&settings_b).expect("parse B")["fast_mode"],
        serde_json::json!(false)
    );
    let local_a = std::fs::read_to_string(coco_config::global_config::local_settings_path(&cwd_a))
        .expect("read local A settings");
    let local_b = std::fs::read_to_string(coco_config::global_config::local_settings_path(&cwd_b))
        .expect("read local B settings");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&local_a).expect("parse local A")["fast_mode"],
        serde_json::json!(false)
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&local_b).expect("parse local B")["fast_mode"],
        serde_json::json!(true)
    );
}

#[tokio::test]
async fn disconnected_session_is_closed_through_explicit_orphan_authority() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        disconnected_orphan_lifecycle_scenario(),
    )
    .await
    .expect("disconnected orphan lifecycle scenario timed out");
}

async fn disconnected_orphan_lifecycle_scenario() {
    let gate = Arc::new(TurnGate {
        started: tokio::sync::Barrier::new(2),
        release: tokio::sync::Semaphore::new(0),
        ignore_cancel: false,
        result_emission: TurnResultEmission::None,
        drop_signal: None,
    });
    let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let started = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start orphan candidate");
    owner
        .turn_start(
            &fixture.handler,
            coco_types::TurnStartParams {
                target: InteractiveTarget {
                    session_id: started.session_id.clone(),
                    surface_id: started.surface_id.expect("surface"),
                },
                prompt: "continue while orphaned".to_string(),
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
            },
        )
        .await
        .expect("start orphan turn");
    gate.started.wait().await;
    let mut pending_callback = fixture
        .server
        .route_server_request_with_reply(
            started.session_id.clone(),
            SurfaceCapability::Interactive,
            None,
            coco_types::ServerRequest::RequestUserInput(coco_types::ServerRequestUserInputParams {
                request_id: "orphan-turn-callback".to_string(),
                prompt: "continue?".to_string(),
                description: None,
                choices: Vec::new(),
                default: None,
            }),
        )
        .expect("route callback before disconnect");
    owner.disconnect();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut pending_callback)
            .await
            .expect("callback fail-closed timeout")
            .is_err(),
        "orphan callback must fail closed"
    );
    gate.release.add_permits(1);
    let observation = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.turn_observations.lock().await.recv(),
    )
    .await
    .expect("turn observation timeout")
    .expect("turn observation");
    assert!(
        !observation.cancelled,
        "disconnect must not cancel the turn"
    );

    let closer = LocalServerClient::connect_local(&fixture.adapter);
    closer
        .session_close(
            &fixture.handler,
            SessionCloseParams {
                target: SessionCloseTarget::Orphaned {
                    target: coco_types::SessionTarget {
                        session_id: started.session_id.clone(),
                    },
                },
            },
        )
        .await
        .expect("close orphan");
    assert!(fixture.server.registry().get(&started.session_id).is_none());
}

#[tokio::test]
async fn close_timeout_aborts_active_turn_and_returns_structured_error() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (turn_drop_tx, turn_drop_rx) = tokio::sync::oneshot::channel();
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: true,
            result_emission: TurnResultEmission::None,
            drop_signal: Some(tokio::sync::Mutex::new(Some(turn_drop_tx))),
        });
        let fixture = fixture_with_turn_gate_and_timeout(
            Some(Arc::clone(&gate)),
            std::time::Duration::from_millis(20),
        )
        .await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start timeout session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.expect("interactive surface"),
        };
        client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: "hang until close timeout".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start hanging turn");
        gate.started.wait().await;

        let error = client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect_err("close must surface drain timeout");
        let ClientError::Server { data, .. } = error else {
            panic!("expected typed server error");
        };
        let data = data.expect("structured close timeout data");
        assert_eq!(
            data.get("kind"),
            Some(&serde_json::json!("session_close_timeout"))
        );
        assert_eq!(data.get("task"), Some(&serde_json::json!("turn_task")));
        assert_eq!(
            data.get("session_id"),
            Some(&serde_json::json!(started.session_id.clone()))
        );
        assert_eq!(data.get("timeout_ms"), Some(&serde_json::json!(20)));

        turn_drop_rx.await.expect("timed-out turn task aborted");
        assert!(fixture.server.registry().get(&started.session_id).is_none());
        assert!(fixture.server.list_live_sessions().is_empty());
    })
    .await
    .expect("close timeout regression timed out");
}

#[tokio::test]
async fn successful_close_has_no_late_session_events_after_completion() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: false,
            result_emission: TurnResultEmission::None,
            drop_signal: None,
        });
        let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start close session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.expect("interactive surface"),
        };

        client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: "close should drain this turn".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start turn");
        gate.started.wait().await;

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect("close session");

        let observation = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.turn_observations.lock().await.recv(),
        )
        .await
        .expect("turn observation timeout")
        .expect("turn observation");
        assert!(
            observation.cancelled,
            "interactive close must cancel the active turn before completing"
        );

        let events_after_close = fixture.observed_outbound.lock().await.clone();
        assert!(
            events_after_close
                .iter()
                .any(|event| event.session_id == started.session_id && event.kind == "turn/ended"),
            "close should drain the active turn before returning"
        );
        assert!(
            events_after_close
                .iter()
                .any(|event| event.session_id == started.session_id
                    && event.kind == "session/result"),
            "close should emit the final session/result before returning"
        );
        let count_after_close = events_after_close
            .iter()
            .filter(|event| event.session_id == started.session_id)
            .count();
        drop(events_after_close);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let final_count = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == started.session_id)
            .count();
        assert_eq!(
            final_count, count_after_close,
            "session close returned before all session events were drained"
        );
        assert!(fixture.server.registry().get(&started.session_id).is_none());
        assert!(fixture.server.list_live_sessions().is_empty());
    })
    .await
    .expect("no-late-session-events regression timed out");
}

#[tokio::test]
async fn close_waits_for_inflight_turn_result_before_final_session_result() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: false,
            result_emission: TurnResultEmission::Standalone,
            drop_signal: None,
        });
        let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start accounting session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.expect("interactive surface"),
        };

        client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: "close should include in-flight accounting".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start turn");
        gate.started.wait().await;

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect("close session");

        let final_result = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .rev()
            .find(|event| event.session_id == started.session_id && event.kind == "session/result")
            .cloned()
            .expect("final session/result event");
        assert_eq!(final_result.session_result_total_turns, Some(1));
        assert_eq!(final_result.session_result_input_tokens, Some(7));
        assert_eq!(final_result.session_result_output_tokens, Some(11));
        assert_eq!(
            final_result.session_result_stop_reason.as_deref(),
            Some("interrupted")
        );
    })
    .await
    .expect("close in-flight accounting regression timed out");
}

#[tokio::test]
async fn embedded_turn_result_is_accounted_without_standalone_session_result() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: false,
            result_emission: TurnResultEmission::Embedded,
            drop_signal: None,
        });
        let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start embedded-result session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.clone().expect("interactive surface"),
        };

        let turn = client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: "embedded result only".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start embedded-result turn");
        gate.started.wait().await;
        gate.release.add_permits(1);

        let ended = wait_for_observed_outbound(&fixture, |event| {
            event.session_id == started.session_id
                && event.kind == "turn/ended"
                && event.turn_id.as_ref() == Some(&turn.turn_id)
        })
        .await;
        assert_eq!(ended.turn_session_result_total_turns, Some(1));
        assert_eq!(
            ended.turn_session_result_stop_reason.as_deref(),
            Some("interrupted")
        );
        assert!(
            fixture.observed_outbound.lock().await.iter().all(|event| {
                event.session_id != started.session_id || event.kind != "session/result"
            }),
            "embedded per-turn result must not require a standalone session/result event"
        );

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect("close embedded-result session");
        let final_result = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .rev()
            .find(|event| event.session_id == started.session_id && event.kind == "session/result")
            .cloned()
            .expect("final session/result event");
        assert_eq!(final_result.session_result_total_turns, Some(1));
        assert_eq!(final_result.session_result_input_tokens, Some(7));
        assert_eq!(final_result.session_result_output_tokens, Some(11));
        assert_eq!(
            final_result.session_result_stop_reason.as_deref(),
            Some("interrupted")
        );
    })
    .await
    .expect("embedded-result accounting regression timed out");
}

#[tokio::test]
async fn embedded_and_standalone_turn_result_is_accounted_once() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: false,
            result_emission: TurnResultEmission::Both,
            drop_signal: None,
        });
        let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start duplicate-result session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.clone().expect("interactive surface"),
        };

        let turn = client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: target.clone(),
                    prompt: "embedded and standalone result".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start duplicate-result turn");
        gate.started.wait().await;
        gate.release.add_permits(1);

        let ended = wait_for_observed_outbound(&fixture, |event| {
            event.session_id == started.session_id
                && event.kind == "turn/ended"
                && event.turn_id.as_ref() == Some(&turn.turn_id)
        })
        .await;
        assert_eq!(ended.turn_session_result_total_turns, Some(1));

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive { target },
                },
            )
            .await
            .expect("close duplicate-result session");
        let final_result = fixture
            .observed_outbound
            .lock()
            .await
            .iter()
            .rev()
            .find(|event| event.session_id == started.session_id && event.kind == "session/result")
            .cloned()
            .expect("final session/result event");
        assert_eq!(
            final_result.session_result_total_turns,
            Some(1),
            "embedded plus standalone per-turn result must not double-count"
        );
        assert_eq!(final_result.session_result_input_tokens, Some(7));
        assert_eq!(final_result.session_result_output_tokens, Some(11));
    })
    .await
    .expect("duplicate-result accounting regression timed out");
}

#[tokio::test]
async fn delete_rejects_live_session_and_close_preserves_transcript_until_delete() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let started = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start deletable session");
        let runtime = fixture
            .server
            .registry()
            .get(&started.session_id)
            .expect("live runtime")
            .into_session();
        let transcript_store = runtime
            .session_manager_handle()
            .store_for(runtime.original_cwd());
        transcript_store
            .append_message(
                started.session_id.as_str(),
                &coco_session::storage::TranscriptEntry {
                    entry_type: "user".to_string(),
                    uuid: "delete-seed".to_string(),
                    parent_uuid: None,
                    logical_parent_uuid: None,
                    session_id: Some(started.session_id.clone()),
                    cwd: runtime.original_cwd().to_string_lossy().into_owned(),
                    timestamp: "2026-07-13T00:00:00Z".to_string(),
                    version: None,
                    git_branch: None,
                    is_sidechain: false,
                    agent_id: None,
                    message: Some(serde_json::json!({"role":"user","content":"seed"})),
                    usage: None,
                    model: None,
                    request_id: None,
                    cost_usd: None,
                    extra: serde_json::Map::new(),
                },
            )
            .expect("seed transcript");
        let transcript_path = transcript_store
            .transcript_path(started.session_id.as_str())
            .expect("disk transcript path");
        let bytes_before_close = std::fs::read(&transcript_path).expect("read seeded transcript");

        let delete_target = coco_types::SessionTarget {
            session_id: started.session_id.clone(),
        };
        let live_delete_error = client
            .session_delete(
                &fixture.handler,
                SessionDeleteParams {
                    target: delete_target.clone(),
                },
            )
            .await
            .expect_err("live delete must fail");
        let ClientError::Server { data, .. } = live_delete_error else {
            panic!("expected server error for live delete");
        };
        assert_eq!(
            data.as_ref().and_then(|data| data.get("kind")),
            Some(&serde_json::json!("SessionStillLive"))
        );

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive {
                        target: InteractiveTarget {
                            session_id: started.session_id.clone(),
                            surface_id: started.surface_id.expect("surface"),
                        },
                    },
                },
            )
            .await
            .expect("close session");
        assert!(fixture.server.registry().get(&started.session_id).is_none());

        let bytes_after_close = std::fs::read(&transcript_path).expect("read closed transcript");
        assert_eq!(
            bytes_after_close, bytes_before_close,
            "session/close must preserve existing transcript bytes exactly"
        );
        let read_after_close = client
            .session_read(
                &fixture.handler,
                SessionReadParams {
                    target: delete_target.clone(),
                    cursor: None,
                    limit: None,
                },
            )
            .await
            .expect("close preserves persisted transcript");
        assert!(!read_after_close.messages.is_empty());

        client
            .session_delete(
                &fixture.handler,
                SessionDeleteParams {
                    target: delete_target.clone(),
                },
            )
            .await
            .expect("delete closed session");
        client
            .session_read(
                &fixture.handler,
                SessionReadParams {
                    target: delete_target,
                    cursor: None,
                    limit: None,
                },
            )
            .await
            .expect_err("delete removes transcript");
        assert!(
            !transcript_path.exists(),
            "session/delete must remove the transcript file"
        );
    })
    .await
    .expect("delete lifecycle scenario timed out");
}

#[tokio::test]
async fn orphan_close_rejects_owned_session_before_turn_or_runtime_side_effects() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let gate = Arc::new(TurnGate {
            started: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
            ignore_cancel: false,
            result_emission: TurnResultEmission::None,
            drop_signal: None,
        });
        let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
        let owner = LocalServerClient::connect_local(&fixture.adapter);
        let started = owner
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start owned session");
        let target = InteractiveTarget {
            session_id: started.session_id.clone(),
            surface_id: started.surface_id.expect("interactive surface"),
        };
        owner
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target,
                    prompt: "must survive rejected orphan close".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start turn");
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.started.wait())
            .await
            .expect("turn reaches barrier");

        let closer = LocalServerClient::connect_local(&fixture.adapter);
        let error = closer
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Orphaned {
                        target: coco_types::SessionTarget {
                            session_id: started.session_id.clone(),
                        },
                    },
                },
            )
            .await
            .expect_err("owned session is not orphan authority");
        let ClientError::Server { data, .. } = error else {
            panic!("expected typed server error");
        };
        assert_eq!(
            data.and_then(|value| value.get("kind").cloned()),
            Some(serde_json::json!("interactive_owner_conflict"))
        );
        assert!(fixture.server.registry().get(&started.session_id).is_some());

        gate.release.add_permits(1);
        let observation = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.turn_observations.lock().await.recv(),
        )
        .await
        .expect("turn observation timeout")
        .expect("turn observation");
        assert!(!observation.cancelled);
    })
    .await
    .expect("orphan close preauthorization timed out");
}

#[tokio::test]
async fn orphaning_cancels_pending_callback_and_rebind_cannot_resolve_it() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let mut owner = LocalServerClient::connect_local(&fixture.adapter);
        let started = owner
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start callback session");
        let mut pending_reply = fixture
            .server
            .route_server_request_with_reply(
                started.session_id.clone(),
                SurfaceCapability::Interactive,
                None,
                coco_types::ServerRequest::RequestUserInput(
                    coco_types::ServerRequestUserInputParams {
                        request_id: "payload-request-id".to_string(),
                        prompt: "continue?".to_string(),
                        description: None,
                        choices: Vec::new(),
                        default: None,
                    },
                ),
            )
            .expect("route callback");
        let delivery = owner
            .server_requests_mut()
            .recv()
            .await
            .expect("callback delivery");

        owner.disconnect();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), &mut pending_reply)
                .await
                .expect("orphan callback cancellation timeout")
                .is_err(),
            "disconnect must fail the pending callback closed"
        );

        let rebound = LocalServerClient::connect_local(&fixture.adapter);
        let rebound_surface = rebound
            .attach_interactive_session(started.session_id.clone(), AttachSurfaceOptions::default())
            .expect("reattach orphaned session");
        let error = rebound
            .user_input_resolve(
                &fixture.handler,
                coco_types::UserInputResolveParams {
                    target: rebound_surface.interactive_target(),
                    request_id: delivery.request_id.as_display(),
                    answer: "yes".to_string(),
                },
            )
            .await
            .expect_err("rebound surface cannot resolve cancelled callback");
        let ClientError::Server { data, .. } = error else {
            panic!("expected typed server error");
        };
        assert_eq!(
            data.and_then(|value| value.get("kind").cloned()),
            Some(serde_json::json!("server_request_not_found"))
        );
    })
    .await
    .expect("orphan callback invalidation timed out");
}

#[tokio::test]
async fn callback_reply_cannot_cross_session_on_the_same_connection() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        callback_authority_matrix_scenario(),
    )
    .await
    .expect("callback authority matrix timed out");
}

async fn callback_authority_matrix_scenario() {
    let fixture = fixture().await;
    let mut owner = LocalServerClient::connect_local(&fixture.adapter);
    let first = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start callback owner A");
    let second = owner
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start sibling B");
    let mut pending_reply = fixture
        .server
        .route_server_request_with_reply(
            first.session_id.clone(),
            SurfaceCapability::Interactive,
            None,
            coco_types::ServerRequest::RequestUserInput(coco_types::ServerRequestUserInputParams {
                request_id: "payload-request-id".to_string(),
                prompt: "continue?".to_string(),
                description: None,
                choices: Vec::new(),
                default: None,
            }),
        )
        .expect("route callback to A");
    let delivery = owner
        .server_requests_mut()
        .recv()
        .await
        .expect("callback delivery");
    let target_a = InteractiveTarget {
        session_id: first.session_id.clone(),
        surface_id: first.surface_id.expect("A surface"),
    };
    let attacker = LocalServerClient::connect_local(&fixture.adapter);
    let error = attacker
        .user_input_resolve(
            &fixture.handler,
            coco_types::UserInputResolveParams {
                target: target_a.clone(),
                request_id: delivery.request_id.as_display(),
                answer: "wrong connection".to_string(),
            },
        )
        .await
        .expect_err("foreign connection cannot resolve A callback");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_wrong_connection"))
    );

    let error = owner
        .user_input_resolve(
            &fixture.handler,
            coco_types::UserInputResolveParams {
                target: target_a.clone(),
                request_id: "wrong-request-id".to_string(),
                answer: "wrong request".to_string(),
            },
        )
        .await
        .expect_err("wrong request id cannot resolve A callback");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("server_request_not_found"))
    );

    let error = owner
        .user_input_resolve(
            &fixture.handler,
            coco_types::UserInputResolveParams {
                target: InteractiveTarget {
                    session_id: first.session_id.clone(),
                    surface_id: coco_types::SurfaceId::from("forged-surface"),
                },
                request_id: delivery.request_id.as_display(),
                answer: "wrong surface".to_string(),
            },
        )
        .await
        .expect_err("foreign surface cannot resolve A callback");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("surface_not_attached"))
    );

    let error = owner
        .user_input_resolve(
            &fixture.handler,
            coco_types::UserInputResolveParams {
                target: InteractiveTarget {
                    session_id: second.session_id,
                    surface_id: second.surface_id.expect("B surface"),
                },
                request_id: delivery.request_id.as_display(),
                answer: "wrong session".to_string(),
            },
        )
        .await
        .expect_err("B cannot resolve A callback");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("server_request_wrong_session"))
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut pending_reply)
            .await
            .is_err(),
        "rejected sibling reply must keep A callback pending"
    );
    owner
        .user_input_resolve(
            &fixture.handler,
            coco_types::UserInputResolveParams {
                target: target_a,
                request_id: delivery.request_id.as_display(),
                answer: "correct".to_string(),
            },
        )
        .await
        .expect("owning target resolves callback");
    let resolved = pending_reply.await.expect("pending callback result");
    assert!(matches!(
        resolved,
        coco_app_server::ServerRequestReply::UserInput(_)
    ));
}

#[tokio::test]
async fn orphan_resume_enforces_callback_requirements_before_rebinding() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        orphan_resume_callback_requirements_scenario(),
    )
    .await
    .expect("orphan resume requirements scenario timed out");
}

async fn orphan_resume_callback_requirements_scenario() {
    let fixture = fixture().await;
    let make_profile = |callback: Option<&str>| InitializeParams {
        hooks: callback.map(|callback| {
            std::collections::HashMap::from([(
                HookEventType::PreToolUse,
                vec![HookCallbackMatcher {
                    matcher: None,
                    hook_callback_ids: vec![callback.to_string()],
                    timeout: None,
                }],
            )])
        }),
        ..Default::default()
    };
    let owner = LocalServerClient::connect_local(&fixture.adapter);
    let owner_handler = fixture
        .handler
        .open(coco_app_server::ConnectionKey::generate());
    owner
        .initialize(
            owner_handler.as_ref(),
            make_profile(Some("required-callback")),
        )
        .await
        .expect("initialize callback owner");
    let started = owner
        .session_start(owner_handler.as_ref(), SessionStartParams::default())
        .await
        .expect("start callback session");
    let runtime = fixture
        .server
        .registry()
        .get(&started.session_id)
        .expect("live callback runtime")
        .into_session();
    runtime
        .session_manager_handle()
        .store_for(runtime.original_cwd())
        .append_message(
            started.session_id.as_str(),
            &coco_session::storage::TranscriptEntry {
                entry_type: "user".to_string(),
                uuid: "resume-seed".to_string(),
                parent_uuid: None,
                logical_parent_uuid: None,
                session_id: Some(started.session_id.clone()),
                cwd: runtime.original_cwd().to_string_lossy().into_owned(),
                timestamp: "2026-07-11T00:00:00Z".to_string(),
                version: None,
                git_branch: None,
                is_sidechain: false,
                agent_id: None,
                message: Some(serde_json::json!({"role":"user","content":"seed"})),
                usage: None,
                model: None,
                request_id: None,
                cost_usd: None,
                extra: serde_json::Map::new(),
            },
        )
        .expect("seed resumable transcript");
    owner.disconnect();

    let incompatible = LocalServerClient::connect_local(&fixture.adapter);
    let incompatible_handler = fixture
        .handler
        .open(coco_app_server::ConnectionKey::generate());
    incompatible
        .initialize(incompatible_handler.as_ref(), make_profile(None))
        .await
        .expect("initialize incompatible connection");
    let error = incompatible
        .session_resume(
            incompatible_handler.as_ref(),
            coco_types::SessionResumeParams {
                target: coco_types::SessionTarget {
                    session_id: started.session_id.clone(),
                },
            },
        )
        .await
        .expect_err("missing callback capability must reject orphan resume");
    let ClientError::Server { data, .. } = error else {
        panic!("expected typed server error");
    };
    assert_eq!(
        data.and_then(|value| value.get("kind").cloned()),
        Some(serde_json::json!("connection_profile_mismatch"))
    );

    let compatible = LocalServerClient::connect_local(&fixture.adapter);
    let compatible_handler = fixture
        .handler
        .open(coco_app_server::ConnectionKey::generate());
    compatible
        .initialize(
            compatible_handler.as_ref(),
            make_profile(Some("required-callback")),
        )
        .await
        .expect("initialize compatible connection");
    let resumed = compatible
        .session_resume(
            compatible_handler.as_ref(),
            coco_types::SessionResumeParams {
                target: coco_types::SessionTarget {
                    session_id: started.session_id.clone(),
                },
            },
        )
        .await
        .expect("compatible profile rebinds orphan");
    assert_eq!(resumed.session.session_id, started.session_id);
    assert!(resumed.surface_id.is_some());
}

#[tokio::test]
async fn session_events_and_replay_never_cross_session_identity() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        event_replay_identity_scenario(),
    )
    .await
    .expect("event replay identity scenario timed out");
}

async fn event_replay_identity_scenario() {
    let fixture = fixture().await;
    let client = LocalServerClient::connect_local(&fixture.adapter);
    let first = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");

    for (session_id, turn_id, seq) in [
        (first.session_id, coco_types::TurnId::from("turn-a"), 1),
        (second.session_id, coco_types::TurnId::from("turn-b"), 1),
    ] {
        fixture.server.route_envelope(SessionEnvelope::durable(
            session_id.clone(),
            None,
            Some(turn_id.clone()),
            seq,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            }),
        ));
        let observer = fixture.adapter.connect();
        let LocalClientSubscribeOutcome::Attached(subscription) = observer
            .subscribe_surface(session_id.clone(), Some(0), AttachSurfaceOptions::default())
            .expect("subscribe with replay")
        else {
            panic!("expected retained replay");
        };
        let replay = subscription.replayed;
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].session_id, session_id);
        assert_eq!(replay[0].turn_id.as_ref(), Some(&turn_id));
    }
}

#[tokio::test]
async fn reload_supervisors_coexist_and_close_reaps_only_the_target_runtime() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let client = LocalServerClient::connect_local(&fixture.adapter);
        let first = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start A");
        let second = client
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start B");
        let runtime_a = fixture
            .server
            .registry()
            .get(&first.session_id)
            .expect("A runtime")
            .into_session();
        let runtime_b = fixture
            .server
            .registry()
            .get(&second.session_id)
            .expect("B runtime")
            .into_session();
        let (drop_a_tx, drop_a_rx) = tokio::sync::oneshot::channel();
        let (drop_b_tx, mut drop_b_rx) = tokio::sync::oneshot::channel();
        runtime_a
            .install_reload_supervisor(tokio::spawn(async move {
                let _signal = DropSignal(Some(drop_a_tx));
                std::future::pending::<()>().await;
            }))
            .await;
        runtime_b
            .install_reload_supervisor(tokio::spawn(async move {
                let _signal = DropSignal(Some(drop_b_tx));
                std::future::pending::<()>().await;
            }))
            .await;

        client
            .session_close(
                &fixture.handler,
                SessionCloseParams {
                    target: SessionCloseTarget::Interactive {
                        target: InteractiveTarget {
                            session_id: first.session_id,
                            surface_id: first.surface_id.expect("A surface"),
                        },
                    },
                },
            )
            .await
            .expect("close A");
        drop_a_rx.await.expect("A reload supervisor stopped");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut drop_b_rx)
                .await
                .is_err(),
            "closing A must not reap B reload supervisor"
        );
        let replaced = client
            .session_replace(
                &fixture.handler,
                coco_types::SessionReplaceParams {
                    source: InteractiveTarget {
                        session_id: second.session_id.clone(),
                        surface_id: second.surface_id.expect("B surface"),
                    },
                    destination: coco_types::SessionReplacement::Fresh(
                        SessionStartParams::default(),
                    ),
                },
            )
            .await
            .expect("replace B");
        drop_b_rx
            .await
            .expect("B reload supervisor stopped by replacement");
        assert!(fixture.server.registry().get(&second.session_id).is_none());
        assert!(
            fixture
                .server
                .registry()
                .get(&replaced.session_id)
                .is_some()
        );
    })
    .await
    .expect("reload supervisor isolation timed out");
}

#[tokio::test]
async fn slow_consumer_disconnects_whole_connection_and_both_sessions_replay_cleanly() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let fixture = fixture().await;
        let stalled = LocalServerClient::connect_local(&fixture.adapter);
        let first = stalled
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start A");
        let second = stalled
            .session_start(&fixture.handler, SessionStartParams::default())
            .await
            .expect("start B");
        for seq in 1..=9 {
            for session_id in [&first.session_id, &second.session_id] {
                fixture.server.route_envelope(SessionEnvelope::durable(
                    session_id.clone(),
                    None,
                    None,
                    seq,
                    CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                        state: SessionState::Running,
                    }),
                ));
            }
        }
        assert!(
            fixture
                .server
                .list_live_sessions()
                .iter()
                .all(|summary| summary.surface_counts.attached == 0),
            "overflow must disconnect the connection and orphan both sessions"
        );

        for session_id in [first.session_id, second.session_id] {
            let observer = fixture.adapter.connect();
            let LocalClientSubscribeOutcome::Attached(subscription) = observer
                .subscribe_surface(session_id.clone(), Some(0), AttachSurfaceOptions::default())
                .expect("reconnect with replay")
            else {
                panic!("retention should cover the complete disconnected interval");
            };
            assert_eq!(subscription.replayed.len(), 9);
            assert!(
                subscription
                    .replayed
                    .iter()
                    .enumerate()
                    .all(|(index, event)| event.session_id == session_id
                        && event.session_seq == Some((index + 1) as i64))
            );
        }
    })
    .await
    .expect("slow-consumer recovery timed out");
}

#[tokio::test]
async fn process_shutdown_drains_both_runtime_backed_sessions() {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        concurrent_shutdown_scenario(),
    )
    .await
    .expect("concurrent shutdown scenario timed out");
}

async fn concurrent_shutdown_scenario() {
    let gate = Arc::new(TurnGate {
        started: tokio::sync::Barrier::new(3),
        release: tokio::sync::Semaphore::new(0),
        ignore_cancel: false,
        result_emission: TurnResultEmission::None,
        drop_signal: None,
    });
    let fixture = fixture_with_turn_gate(Some(Arc::clone(&gate))).await;
    let client = LocalServerClient::connect_local(&fixture.adapter);
    let first = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start A");
    let second = client
        .session_start(&fixture.handler, SessionStartParams::default())
        .await
        .expect("start B");
    for started in [&first, &second] {
        client
            .turn_start(
                &fixture.handler,
                coco_types::TurnStartParams {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id: started.surface_id.clone().expect("surface"),
                    },
                    prompt: "shutdown-concurrency".to_string(),
                    history_override: Vec::new(),
                    images: Vec::new(),
                    slash_metadata: None,
                    model_selection: None,
                    permission_mode: None,
                    thinking_level: None,
                },
            )
            .await
            .expect("start turn");
    }
    gate.started.wait().await;

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        shutdown_local_app_server_sessions(
            Arc::clone(&fixture.server),
            Arc::clone(&fixture.state),
            std::time::Duration::from_secs(5),
        ),
    )
    .await
    .expect("shutdown must close A and B concurrently")
    .expect("drain all sessions");

    assert!(fixture.server.registry().get(&first.session_id).is_none());
    assert!(fixture.server.registry().get(&second.session_id).is_none());
    assert!(fixture.server.list_live_sessions().is_empty());
}
