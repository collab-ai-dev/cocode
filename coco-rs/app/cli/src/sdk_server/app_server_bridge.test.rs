use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::TimeZone;
use clap::Parser;
use coco_app_server::AppServer;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::JsonRpcAdapter;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server::SurfaceRole;
use coco_app_server_transport::JsonRpcErrorObject as TransportJsonRpcErrorObject;
use coco_app_server_transport::JsonRpcErrorResponse as TransportJsonRpcErrorResponse;
use coco_app_server_transport::JsonRpcFrame;
use coco_app_server_transport::JsonRpcId;
use coco_app_server_transport::JsonRpcSuccess;
use coco_bridge::ControlRequestHandler;
use coco_hub_connector::HubConnectorWorker;
use coco_hub_connector::HubConnectorWorkerConfig;
use coco_hub_connector::protocol::AnnounceAckFrame;
use coco_hub_connector::protocol::AnnounceFrame;
use coco_hub_connector::protocol::BatchAckFrame;
use coco_hub_connector::protocol::BatchFrame;
use coco_hub_connector::protocol::HubFrame;
use coco_hub_connector::protocol::SUBPROTOCOL_V2;
use coco_types::ClientRequest;
use coco_types::CoreEvent;
use coco_types::JSONRPC_VERSION;
use coco_types::JsonRpcMessage;
use coco_types::JsonRpcRequest;
use coco_types::JsonRpcResponse;
use coco_types::RequestId;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionState;
use futures::SinkExt;
use futures::StreamExt;
use http::HeaderValue;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::*;
use crate::sdk_server::handlers::ActiveTurnHandles;
use crate::sdk_server::handlers::SessionMetadata;
use crate::sdk_server::handlers::TurnHandoff;
use crate::sdk_server::handlers::TurnRunner;
use crate::sdk_server::transport::InMemoryTransport;
use crate::sdk_server::transport::SdkJsonRpcFrameError;
use crate::sdk_server::transport::SdkTransport;
use crate::sdk_server::transport::json_rpc_frame_to_message;
use crate::sdk_server::transport::json_rpc_message_to_frame;

struct EndingTurnRunner;

impl TurnRunner for EndingTurnRunner {
    fn run_turn<'a>(
        &'a self,
        _params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        _handoff: TurnHandoff,
        event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnStarted(
                    coco_types::TurnStartedParams {
                        turn_id: turn_id.clone(),
                    },
                )))
                .await
                .ok();
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnEnded(
                    coco_types::TurnEndedParams::completed(
                        turn_id,
                        Some(coco_types::TokenUsage::default()),
                        Some(coco_messages::StopReason::EndTurn),
                    ),
                )))
                .await
                .ok();
            Ok(())
        })
    }
}

struct AccountingTurnRunner;

impl TurnRunner for AccountingTurnRunner {
    fn run_turn<'a>(
        &'a self,
        _params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        handoff: TurnHandoff,
        event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnStarted(
                    coco_types::TurnStartedParams {
                        turn_id: turn_id.clone(),
                    },
                )))
                .await
                .ok();
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::SessionResult(
                    Box::new(coco_types::SessionResultParams {
                        session_id: handoff.session_id,
                        total_turns: 1,
                        duration_ms: 7,
                        duration_api_ms: 3,
                        is_error: false,
                        stop_reason: "end_turn".to_string(),
                        total_cost_usd: 0.0,
                        usage: coco_types::TokenUsage {
                            input_tokens: coco_types::InputTokens {
                                total: 11,
                                ..Default::default()
                            },
                            output_tokens: coco_types::OutputTokens {
                                total: 5,
                                ..Default::default()
                            },
                        },
                        model_usage: std::collections::HashMap::new(),
                        permission_denials: Vec::new(),
                        result: Some("completed".to_string()),
                        errors: Vec::new(),
                        structured_output: None,
                        fast_mode_state: None,
                        num_api_calls: None,
                    }),
                )))
                .await
                .ok();
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnEnded(
                    coco_types::TurnEndedParams::completed(
                        turn_id,
                        Some(coco_types::TokenUsage::default()),
                        Some(coco_messages::StopReason::EndTurn),
                    ),
                )))
                .await
                .ok();
            Ok(())
        })
    }
}

fn test_runtime_config(home: &Path) -> coco_config::RuntimeConfig {
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(coco_config::RoleSlots::new(
                    coco_types::ProviderModelSelection {
                        provider: "anthropic".into(),
                        model_id: "claude-opus-4-7".into(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: HashMap::new(),
        source_paths: HashMap::new(),
    };
    coco_config::build_runtime_config_with(
        settings,
        coco_config::EnvSnapshot::default(),
        coco_config::RuntimeOverrides::default(),
        coco_config::CatalogPaths::empty_in(home),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config")
}

fn test_runtime_factory(
    home: &Path,
    runtime_config: Arc<coco_config::RuntimeConfig>,
    session_manager: Arc<coco_session::SessionManager>,
) -> crate::session_runtime::SessionRuntimeFactory {
    let cli = crate::Cli::try_parse_from(["coco"]).expect("parse cli");
    let model_id = crate::headless::resolve_main_model(&runtime_config).model_id;
    crate::session_runtime::SessionRuntimeFactory::new(
        crate::session_runtime::SessionRuntimeFactoryOpts {
            cli: Arc::new(cli),
            bootstrap_source:
                crate::session_runtime::SessionRuntimeBootstrapSource::startup_snapshot(
                    crate::session_runtime::SessionRuntimeBootstrap {
                        runtime_config,
                        model_id,
                        system_prompt: "test".to_string(),
                        permission_mode_availability:
                            coco_types::PermissionModeAvailability::default(),
                        permission_mode: coco_types::PermissionMode::default(),
                        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                            coco_commands::CommandRegistry::default(),
                        ))),
                        skill_manager: Arc::new(coco_skills::SkillManager::new()),
                        project_services: Arc::new(crate::project_services::ProjectServices::load(
                            home, home,
                        )),
                        agent_search_paths:
                            coco_subagent::definition_store::AgentSearchPaths::empty(),
                    },
                ),
            cwd: home.to_path_buf(),
            model_runtimes: None,
            tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
            session_manager,
            fast_model_spec: None,
            permission_bridge: None,
            process_runtime: crate::process_runtime::ProcessRuntime::global(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            is_non_interactive: true,
        },
    )
}

#[tokio::test]
async fn app_server_sdk_handler_dispatches_into_existing_handlers() {
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let handler = AppServerSdkHandler::new(Arc::new(SdkServerState::default()), notif_tx);

    let result = handler
        .handle_json_rpc_request(
            JsonRpcRequestContext {
                connection: coco_app_server::ConnectionKey::generate(),
            },
            ClientRequest::KeepAlive,
        )
        .await
        .expect("keepAlive succeeds");

    assert_eq!(result, serde_json::Value::Null);
}

#[tokio::test]
async fn app_server_sdk_handler_dispatches_local_requests_into_existing_handlers() {
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let handler = AppServerSdkHandler::new(Arc::new(SdkServerState::default()), notif_tx);

    let result = handler
        .handle_local_client_request(
            LocalClientRequestContext::new(coco_app_server::ConnectionKey::generate()),
            ClientRequest::KeepAlive,
        )
        .await
        .expect("keepAlive succeeds");

    assert_eq!(result, serde_json::Value::Null);
}

#[tokio::test]
async fn app_server_local_bridge_dispatches_requests_and_reads_surface_events() {
    let state = Arc::new(SdkServerState::default());
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);

    let started = bridge
        .client()
        .session_start(
            bridge.handler(),
            coco_types::SessionStartParams {
                cwd: Some(".".to_string()),
                model: Some("test-model".to_string()),
                ..coco_types::SessionStartParams::default()
            },
        )
        .await
        .expect("session/start succeeds");

    assert_eq!(
        state
            .session_metadata_snapshot(&started.session_id)
            .unwrap()
            .model,
        "test-model"
    );
    assert!(
        state
            .session_handoff_snapshot(&started.session_id)
            .is_some()
    );

    let surface_id = started.surface_id.clone().expect("start surface id");
    let app_server = Arc::clone(bridge.app_server());
    app_server.route_envelope(SessionEnvelope::ephemeral(
        started.session_id.clone(),
        None,
        None,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    ));

    let delivered = bridge
        .client_mut()
        .events_mut()
        .recv()
        .await
        .expect("surface event");
    assert_eq!(delivered.surface_id, surface_id);
    assert_eq!(delivered.envelope.session_id, started.session_id);
}

#[tokio::test]
async fn app_server_local_bridge_session_start_replaces_startup_live_slot() {
    let state = Arc::new(SdkServerState::default());
    let bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let startup_session_id =
        SessionId::try_new("sess-local-startup-placeholder").expect("valid startup session id");
    register_local_app_server_session(
        bridge.app_server(),
        LocalAppSessionHandle::snapshot(startup_session_id.clone()),
    )
    .await
    .expect("register startup slot");

    let started = bridge
        .client()
        .session_start(
            bridge.handler(),
            coco_types::SessionStartParams {
                cwd: Some(".".to_string()),
                model: Some("test-model".to_string()),
                ..coco_types::SessionStartParams::default()
            },
        )
        .await
        .expect("session/start succeeds");

    let live = bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, started.session_id);
    assert_ne!(live[0].session_id, startup_session_id);
}

#[tokio::test]
async fn app_server_sdk_session_start_uses_runtime_replacement_context() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime_config = Arc::new(test_runtime_config(home.path()));
    let session_manager = Arc::new(coco_session::SessionManager::new(
        home.path().join("sessions"),
    ));
    let factory = test_runtime_factory(
        home.path(),
        Arc::clone(&runtime_config),
        Arc::clone(&session_manager),
    );
    let startup_session_id =
        SessionId::try_new("sess-sdk-startup-placeholder").expect("valid startup session id");
    let startup_runtime = factory
        .build_with_session_id(startup_session_id.clone())
        .await
        .expect("startup runtime");

    let state = Arc::new(SdkServerState::default());
    {
        state.install_session_runtime(startup_runtime.clone()).await;
    }
    state
        .install_runtime_replacement(RuntimeReplacementContext {
            runtime_factory: factory,
            process_runtime: crate::process_runtime::ProcessRuntime::global(),
            cwd: home.path().to_path_buf(),
            requires_structured_output: false,
        })
        .await;

    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::from_runtime(startup_session_id.clone(), startup_runtime.clone()),
    )
    .await
    .expect("register startup runtime");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let client = ServerClient::connect_local(&adapter);
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let handler =
        AppServerSdkHandler::with_local_app_server(Arc::clone(&state), notif_tx, app_server);

    let started = client
        .session_start(
            &handler,
            coco_types::SessionStartParams {
                cwd: Some(home.path().to_string_lossy().into_owned()),
                model: Some("test-model".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session/start succeeds");

    assert_ne!(started.session_id, startup_session_id);
    let current_runtime = state
        .session_runtime_snapshot()
        .await
        .expect("runtime replaced");
    assert_eq!(
        current_runtime.current_typed_session_id().await,
        started.session_id
    );
    assert!(!Arc::ptr_eq(
        startup_runtime.runtime(),
        current_runtime.runtime()
    ));
    let handoff = state
        .session_handoff_snapshot(&started.session_id)
        .expect("SDK handoff installed");
    assert!(Arc::ptr_eq(
        &handoff.app_state,
        current_runtime.runtime().app_state()
    ));
    crate::sdk_server::SdkBridgeControlHandler::new(Arc::clone(&state))
        .handle(coco_bridge::ControlRequest::SetPermissionMode {
            mode: coco_types::PermissionMode::AcceptEdits,
        })
        .await
        .expect("bridge control uses runtime session id");
    let app_state = handoff.app_state.read().await;
    assert_eq!(
        app_state.permissions.mode,
        Some(coco_types::PermissionMode::AcceptEdits)
    );
}

#[tokio::test]
async fn app_server_sdk_session_resume_uses_scoped_runtime_replacement_state() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime_config = Arc::new(test_runtime_config(home.path()));
    let session_manager = Arc::new(coco_session::SessionManager::new(
        home.path().join("sessions"),
    ));
    let factory = test_runtime_factory(
        home.path(),
        Arc::clone(&runtime_config),
        Arc::clone(&session_manager),
    );
    let startup_session_id =
        SessionId::try_new("sess-sdk-resume-startup").expect("valid startup session id");
    let startup_runtime = factory
        .build_with_session_id(startup_session_id.clone())
        .await
        .expect("startup runtime");
    let resumed_session_id =
        SessionId::try_new("sess-sdk-resume-target").expect("valid resumed session id");
    append_bridge_transcript_message(
        &session_manager,
        &resumed_session_id,
        home.path(),
        1,
        "user",
        "resume me",
    );

    let state = Arc::new(SdkServerState::default());
    {
        state.install_session_runtime(startup_runtime.clone()).await;
        state
            .install_session_manager(Arc::clone(&session_manager))
            .await;
    }
    state
        .install_runtime_replacement(RuntimeReplacementContext {
            runtime_factory: factory,
            process_runtime: crate::process_runtime::ProcessRuntime::global(),
            cwd: home.path().to_path_buf(),
            requires_structured_output: false,
        })
        .await;

    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::from_runtime(startup_session_id.clone(), startup_runtime.clone()),
    )
    .await
    .expect("register startup runtime");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let client = ServerClient::connect_local(&adapter);
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let handler =
        AppServerSdkHandler::with_local_app_server(Arc::clone(&state), notif_tx, app_server);

    let resumed = client
        .session_resume(
            &handler,
            coco_types::SessionResumeParams {
                session_id: resumed_session_id.clone(),
            },
        )
        .await
        .expect("session/resume succeeds");

    assert_eq!(resumed.session.session_id, resumed_session_id);
    assert!(resumed.surface_id.is_some());
    let current_runtime = state
        .session_runtime_snapshot()
        .await
        .expect("runtime replaced");
    assert_eq!(
        current_runtime.current_typed_session_id().await,
        resumed_session_id
    );
    assert!(!Arc::ptr_eq(
        startup_runtime.runtime(),
        current_runtime.runtime()
    ));
    let handoff = state
        .session_handoff_snapshot(&resumed_session_id)
        .expect("resumed handoff installed");
    assert_eq!(handoff.history.lock().await.len(), 1);
    assert!(Arc::ptr_eq(
        &handoff.app_state,
        current_runtime.runtime().app_state()
    ));
}

#[tokio::test]
async fn app_server_local_bridge_archives_registered_surfaces() {
    let state = Arc::new(SdkServerState::default());
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);

    let started = bridge
        .client()
        .session_start(
            bridge.handler(),
            coco_types::SessionStartParams {
                cwd: Some(".".to_string()),
                model: Some("test-model".to_string()),
                ..coco_types::SessionStartParams::default()
            },
        )
        .await
        .expect("session/start succeeds");
    let surface_id = started.surface_id.clone().expect("start surface id");

    bridge
        .client()
        .session_archive(
            bridge.handler(),
            coco_types::SessionArchiveParams {
                session_id: started.session_id.clone(),
            },
        )
        .await
        .expect("session/archive succeeds");
    assert!(bridge.app_server().list_live_sessions().is_empty());

    let lifecycle = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        bridge.client_mut().lifecycle_mut().recv(),
    )
    .await
    .expect("session ended lifecycle")
    .expect("lifecycle channel open");
    assert_eq!(lifecycle.surface_id, surface_id);
    assert_eq!(
        lifecycle.effect.kind,
        coco_app_server::SurfaceLifecycleEffectKind::SessionEnded {
            session_id: started.session_id.clone(),
        }
    );
    assert!(
        state
            .session_handoff_snapshot(&started.session_id)
            .is_none()
    );
}

#[tokio::test]
async fn local_app_server_close_cancels_and_drains_matching_sdk_session() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let state = Arc::new(SdkServerState::default());
    let session_id = SessionId::try_new("sess-local-close-drain").expect("valid session id");
    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::snapshot(session_id.clone()),
    )
    .await
    .expect("register local session");

    let cancel = CancellationToken::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_task = Arc::clone(&cancelled);
    let cancel_for_task = cancel.clone();
    let turn_task = tokio::spawn(async move {
        cancel_for_task.cancelled().await;
        cancelled_for_task.store(true, Ordering::SeqCst);
    });
    let forwarder_task = tokio::spawn(async {});
    state
        .install_test_session_state(
            session_id.clone(),
            SessionMetadata {
                cwd: "/tmp".to_string(),
                model: "test-model".to_string(),
            },
        )
        .await;
    state.install_active_turn(
        session_id.clone(),
        ActiveTurnHandles {
            cancel_token: cancel,
            turn_task,
            forwarder_task,
        },
    );

    close_local_app_server_session(
        Arc::clone(&app_server),
        Arc::clone(&state),
        session_id.clone(),
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("close local session");

    assert!(cancelled.load(Ordering::SeqCst));
    assert!(state.session_handoff_snapshot(&session_id).is_none());
    assert!(app_server.list_live_sessions().is_empty());
}

#[tokio::test]
async fn local_app_server_shutdown_drains_all_registered_sessions() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(2, 8));
    let state = Arc::new(SdkServerState::default());
    let session_a = SessionId::try_new("sess-local-shutdown-a").expect("valid session id");
    let session_b = SessionId::try_new("sess-local-shutdown-b").expect("valid session id");
    for session_id in [session_a.clone(), session_b.clone()] {
        register_local_app_server_session(
            &app_server,
            LocalAppSessionHandle::snapshot(session_id.clone()),
        )
        .await
        .expect("register local session");
        state
            .install_test_session_state(
                session_id,
                SessionMetadata {
                    cwd: "/tmp".to_string(),
                    model: "test-model".to_string(),
                },
            )
            .await;
    }

    let cancel_a = CancellationToken::new();
    let cancelled_a = Arc::new(AtomicBool::new(false));
    let cancelled_a_for_task = Arc::clone(&cancelled_a);
    let cancel_a_for_task = cancel_a.clone();
    let turn_task_a = tokio::spawn(async move {
        cancel_a_for_task.cancelled().await;
        cancelled_a_for_task.store(true, Ordering::SeqCst);
    });
    state.install_active_turn(
        session_a.clone(),
        ActiveTurnHandles {
            cancel_token: cancel_a,
            turn_task: turn_task_a,
            forwarder_task: tokio::spawn(async {}),
        },
    );

    let cancel_b = CancellationToken::new();
    let cancelled_b = Arc::new(AtomicBool::new(false));
    let cancelled_b_for_task = Arc::clone(&cancelled_b);
    let cancel_b_for_task = cancel_b.clone();
    let turn_task_b = tokio::spawn(async move {
        cancel_b_for_task.cancelled().await;
        cancelled_b_for_task.store(true, Ordering::SeqCst);
    });
    state.install_active_turn(
        session_b.clone(),
        ActiveTurnHandles {
            cancel_token: cancel_b,
            turn_task: turn_task_b,
            forwarder_task: tokio::spawn(async {}),
        },
    );

    shutdown_local_app_server_sessions(
        Arc::clone(&app_server),
        Arc::clone(&state),
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("shutdown local sessions");

    assert!(cancelled_a.load(Ordering::SeqCst));
    assert!(cancelled_b.load(Ordering::SeqCst));
    assert!(state.session_handoff_snapshot(&session_a).is_none());
    assert!(state.session_handoff_snapshot(&session_b).is_none());
    assert_eq!(app_server.registry().slot_count(), 0);
}

#[tokio::test]
async fn local_app_server_close_clears_scoped_state_without_sdk_slot() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let state = Arc::new(SdkServerState::default());
    let session_id = SessionId::try_new("sess-local-close-scoped").expect("valid session id");
    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::snapshot(session_id.clone()),
    )
    .await
    .expect("register local session");
    state.set_session_handoff(session_id.clone(), SessionHandoffState::new());
    assert!(state.session_handoff_snapshot(&session_id).is_some());

    close_local_app_server_session(
        Arc::clone(&app_server),
        Arc::clone(&state),
        session_id.clone(),
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("close local session");

    assert!(app_server.list_live_sessions().is_empty());
    assert!(state.session_handoff_snapshot(&session_id).is_none());
}

#[tokio::test]
async fn local_bridge_runtime_load_failure_removes_loading_slot() {
    let bridge = AppServerLocalBridge::with_channel_capacity(
        Arc::new(SdkServerState::default()),
        /*channel_capacity*/ 8,
    );
    let session_id = SessionId::try_new("sess-local-runtime-load-fails").expect("valid session id");

    let result = bridge
        .load_session_runtime(session_id.clone(), async {
            Err::<crate::session_runtime::SessionHandle, _>(anyhow::anyhow!("factory failed"))
        })
        .await;
    let error = match result {
        Ok(_) => panic!("runtime factory failure should be reported"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("factory failed"),
        "unexpected error: {error:#}"
    );
    assert!(bridge.app_server().list_live_sessions().is_empty());
}

#[tokio::test]
async fn local_bridge_runtime_replace_failure_keeps_old_interactive_slot() {
    let mut bridge = AppServerLocalBridge::with_channel_capacity(
        Arc::new(SdkServerState::default()),
        /*channel_capacity*/ 8,
    );
    let old_session_id =
        SessionId::try_new("sess-local-runtime-replace-old").expect("valid old session id");
    let new_session_id =
        SessionId::try_new("sess-local-runtime-replace-new").expect("valid new session id");
    register_local_app_server_session(
        bridge.app_server(),
        LocalAppSessionHandle::snapshot(old_session_id.clone()),
    )
    .await
    .expect("register old session");
    bridge
        .ensure_interactive_surface(old_session_id.clone())
        .expect("attach old interactive surface");
    let old_surface_id = bridge
        .interactive_surface
        .as_ref()
        .expect("interactive surface")
        .surface_id()
        .clone();

    let result = bridge
        .replace_session_runtime(old_session_id.clone(), new_session_id.clone(), async {
            Err::<crate::session_runtime::SessionHandle, _>(anyhow::anyhow!(
                "replace factory failed"
            ))
        })
        .await;
    let error = match result {
        Ok(_) => panic!("runtime replace factory failure should be reported"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("replace factory failed"),
        "unexpected error: {error:#}"
    );
    let live = bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, old_session_id);
    let routing = bridge
        .app_server()
        .routing()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        routing.surface_session(&old_surface_id),
        Some(&old_session_id)
    );
}

#[tokio::test]
async fn local_bridge_replace_factory_returns_constructed_handle() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let state = Arc::new(SdkServerState::default());
    let old_session_id =
        SessionId::try_new("sess-local-replace-return-old").expect("valid old session id");
    let new_session_id =
        SessionId::try_new("sess-local-replace-return-new").expect("valid new session id");
    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::snapshot(old_session_id.clone()),
    )
    .await
    .expect("register old session");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let connection = adapter.connect();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..Default::default()
    };
    let surface = connection
        .attach_surface(old_session_id.clone(), options)
        .expect("attach old interactive surface");

    let (returned, returned_surface) = replace_local_app_server_session_with_factory(
        Arc::clone(&app_server),
        state,
        old_session_id.clone(),
        new_session_id.clone(),
        {
            let new_session_id = new_session_id.clone();
            async move {
                Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                    LocalAppSessionHandle::snapshot(new_session_id),
                )
            }
        },
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("replace succeeds")
    .expect("interactive replace returns caller surface");

    assert_eq!(returned.session_id(), &new_session_id);
    assert_eq!(returned_surface, surface.surface_id);
    assert!(app_server.registry().get(&old_session_id).is_none());
    assert_eq!(
        app_server
            .registry()
            .get(&new_session_id)
            .expect("new live handle")
            .session_id(),
        &new_session_id
    );
}

#[tokio::test]
async fn local_bridge_runtime_replace_failure_keeps_old_detached_slot() {
    let bridge = AppServerLocalBridge::with_channel_capacity(
        Arc::new(SdkServerState::default()),
        /*channel_capacity*/ 8,
    );
    let old_session_id =
        SessionId::try_new("sess-local-detached-replace-old").expect("valid old session id");
    let new_session_id =
        SessionId::try_new("sess-local-detached-replace-new").expect("valid new session id");
    register_local_app_server_session(
        bridge.app_server(),
        LocalAppSessionHandle::snapshot(old_session_id.clone()),
    )
    .await
    .expect("register old session");

    let result = bridge
        .replace_detached_session_runtime(old_session_id.clone(), new_session_id, async {
            Err::<crate::session_runtime::SessionHandle, _>(anyhow::anyhow!(
                "detached replace factory failed"
            ))
        })
        .await;
    let error = match result {
        Ok(_) => panic!("detached runtime replace factory failure should be reported"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("detached replace factory failed"),
        "unexpected error: {error:#}"
    );
    let live = bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, old_session_id);
}

#[tokio::test]
async fn local_lifecycle_resume_replaces_detached_live_session_before_attaching_new_surface() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let old_session_id = SessionId::try_new("sess-local-resume-old").expect("valid old session id");
    let new_session_id = SessionId::try_new("sess-local-resume-new").expect("valid new session id");

    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::snapshot(old_session_id.clone()),
    )
    .await
    .expect("register old session");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let mut connection = adapter.connect();
    let surface = connection
        .attach_surface(old_session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach old surface");

    let result = serde_json::to_value(coco_types::SessionResumeResult {
        session: coco_types::SdkSessionSummary {
            session_id: new_session_id.clone(),
            model: "test-model".to_string(),
            cwd: "/tmp/resumed".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: None,
            title: None,
            message_count: 0,
            total_tokens: 0,
        },
        surface_id: None,
    })
    .expect("encode resume result");

    let new_surface_id = apply_local_lifecycle_request(
        Arc::clone(&app_server),
        Arc::new(SdkServerState::default()),
        LocalLifecycleRequest::Resume {
            connection: connection.connection_key(),
            live_before: vec![old_session_id.clone()],
        },
        &result,
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("apply resume lifecycle")
    .expect("new surface id");

    let live = app_server.list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, new_session_id);
    {
        let routing = app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(routing.surface_session(&surface.surface_id), None);
        assert_eq!(
            routing.surface_session(&new_surface_id),
            Some(&new_session_id)
        );
        assert_eq!(
            routing.interactive_owner(&new_session_id),
            Some(&new_surface_id)
        );
    }
    let observed_session_id = new_session_id.clone();
    let live_handle = app_server
        .spawn_load(new_session_id.clone(), async move {
            Ok::<LocalAppSessionHandle, coco_app_server::RegistryError>(
                LocalAppSessionHandle::snapshot(observed_session_id),
            )
        })
        .expect("observe live handle");
    let coco_app_server::AppLoadStart::Live(handle) = live_handle else {
        panic!("expected live local session handle");
    };
    assert_eq!(handle.session_id(), &new_session_id);

    let lifecycle = tokio::time::timeout(Duration::from_secs(1), connection.lifecycle_mut().recv())
        .await
        .expect("session ended lifecycle")
        .expect("lifecycle channel open");
    assert_eq!(lifecycle.surface_id, surface.surface_id);
    assert_eq!(
        lifecycle.effect.kind,
        coco_app_server::SurfaceLifecycleEffectKind::SessionEnded {
            session_id: old_session_id,
        }
    );
}

#[tokio::test]
async fn local_lifecycle_resume_replaces_interactive_live_session() {
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let old_session_id =
        SessionId::try_new("sess-local-replace-old").expect("valid old session id");
    let new_session_id =
        SessionId::try_new("sess-local-replace-new").expect("valid new session id");

    register_local_app_server_session(
        &app_server,
        LocalAppSessionHandle::snapshot(old_session_id.clone()),
    )
    .await
    .expect("register old session");
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let mut connection = adapter.connect();
    let options = AttachSurfaceOptions {
        role: SurfaceRole::Interactive,
        ..Default::default()
    };
    let surface = connection
        .attach_surface(old_session_id.clone(), options)
        .expect("attach old interactive surface");

    let result = serde_json::to_value(coco_types::SessionResumeResult {
        session: coco_types::SdkSessionSummary {
            session_id: new_session_id.clone(),
            model: "test-model".to_string(),
            cwd: "/tmp/resumed".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: None,
            title: None,
            message_count: 0,
            total_tokens: 0,
        },
        surface_id: None,
    })
    .expect("encode resume result");

    apply_local_lifecycle_request(
        Arc::clone(&app_server),
        Arc::new(SdkServerState::default()),
        LocalLifecycleRequest::Resume {
            connection: connection.connection_key(),
            live_before: vec![old_session_id.clone()],
        },
        &result,
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("apply resume lifecycle");

    let live = app_server.list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, new_session_id);
    {
        let routing = app_server
            .routing()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            routing.surface_session(&surface.surface_id),
            Some(&new_session_id)
        );
        assert_eq!(
            routing.interactive_owner(&new_session_id),
            Some(&surface.surface_id)
        );
        assert_eq!(routing.interactive_owner(&old_session_id), None);
    }

    let lifecycle = tokio::time::timeout(Duration::from_secs(1), connection.lifecycle_mut().recv())
        .await
        .expect("session started lifecycle")
        .expect("lifecycle channel open");
    assert_eq!(lifecycle.surface_id, surface.surface_id);
    assert_eq!(
        lifecycle.effect.kind,
        coco_app_server::SurfaceLifecycleEffectKind::SessionStarted {
            session_id: new_session_id,
        }
    );
}

#[tokio::test]
async fn app_server_local_bridge_can_install_existing_session_snapshot() {
    let state = Arc::new(SdkServerState::default());
    let bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let session_id =
        coco_types::SessionId::try_new("sess-existing-local").expect("valid session id");

    bridge
        .install_session_snapshot(
            session_id.clone(),
            "/tmp/existing-session",
            "existing-model",
        )
        .await;

    let metadata = state.session_metadata_snapshot(&session_id).unwrap();
    assert_eq!(metadata.cwd, "/tmp/existing-session");
    assert_eq!(metadata.model, "existing-model");
    assert!(state.session_handoff_snapshot(&session_id).is_some());
}

#[tokio::test]
async fn current_session_result_prefers_interactive_surface_over_sdk_slot() {
    let state = Arc::new(SdkServerState::default());
    state
        .install_turn_runner(Arc::new(AccountingTurnRunner))
        .await;
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let old_session_id =
        coco_types::SessionId::try_new("sess-result-old").expect("valid old session id");
    let new_session_id =
        coco_types::SessionId::try_new("sess-result-new").expect("valid new session id");
    bridge
        .install_session_snapshot(old_session_id.clone(), "/tmp/old-session", "old-model")
        .await;
    bridge
        .ensure_interactive_surface(old_session_id.clone())
        .expect("attach old interactive surface");
    replace_local_app_server_session(
        Arc::clone(bridge.app_server()),
        Arc::clone(&state),
        old_session_id.clone(),
        LocalAppSessionHandle::snapshot(new_session_id.clone()),
        APP_SERVER_TURN_DRAIN_TIMEOUT,
    )
    .await
    .expect("replace session")
    .expect("calling surface replaced");

    let stale_slot_session_id =
        coco_types::SessionId::try_new("sess-result-stale-slot").expect("valid stale session id");
    state
        .install_test_session_state(
            stale_slot_session_id.clone(),
            SessionMetadata {
                cwd: "/tmp/stale-slot".to_string(),
                model: "stale-model".to_string(),
            },
        )
        .await;
    state.set_session_handoff(new_session_id.clone(), SessionHandoffState::new());
    bridge
        .client()
        .update_env(
            bridge.handler(),
            coco_types::UpdateEnvParams {
                env: HashMap::from([("SCOPED".to_string(), "1".to_string())]),
            },
        )
        .await
        .expect("scoped update env succeeds with stale current-session fallback");
    bridge
        .start_turn_and_wait_for_end(
            new_session_id.clone(),
            coco_types::TurnStartParams {
                prompt: "scoped turn".into(),
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
            },
        )
        .await
        .expect("scoped turn succeeds with stale current-session fallback");
    let result = bridge
        .current_session_result()
        .await
        .expect("session result");
    assert_eq!(result.session_id, new_session_id);
    assert_eq!(result.total_turns, 1);
    assert_eq!(result.usage.input_tokens.total, 11);
    assert_eq!(result.usage.output_tokens.total, 5);

    let surface = bridge
        .interactive_surface
        .as_ref()
        .expect("interactive surface")
        .clone();
    bridge
        .client()
        .session_archive(
            bridge.handler(),
            coco_types::SessionArchiveParams {
                session_id: new_session_id.clone(),
            },
        )
        .await
        .expect("scoped archive succeeds through keyed SDK state");
    let archive_event = tokio::time::timeout(
        Duration::from_secs(1),
        bridge.client_mut().next_session_event(&surface),
    )
    .await
    .expect("scoped archive event delivered")
    .expect("session event channel open");
    assert_eq!(archive_event.session_id, new_session_id);
    assert!(matches!(
        archive_event.event,
        CoreEvent::Protocol(ServerNotification::SessionResult(params))
            if params.session_id == new_session_id
    ));
    assert!(bridge.app_server().list_live_sessions().is_empty());
}

#[tokio::test]
async fn app_server_local_bridge_drains_handler_events_to_surface_channel() {
    let state = Arc::new(SdkServerState::default());
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let session_id =
        coco_types::SessionId::try_new("sess-local-event-drain").expect("valid session id");
    bridge
        .install_session_snapshot(session_id.clone(), ".", "test-model")
        .await;
    bridge
        .ensure_interactive_surface(session_id)
        .expect("attach local surface");

    bridge
        .client()
        .set_permission_mode(
            bridge.handler(),
            coco_types::SetPermissionModeParams {
                mode: coco_types::PermissionMode::Plan,
            },
        )
        .await
        .expect("set permission mode");

    let (event_tx, mut event_rx) = mpsc::channel(8);
    for _ in 0..20 {
        bridge.drain_interactive_events_to(&event_tx).await;
        if let Ok(event) = event_rx.try_recv() {
            assert!(matches!(
                event,
                CoreEvent::Protocol(ServerNotification::PermissionModeChanged(params))
                    if params.mode == coco_types::PermissionMode::Plan
            ));
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("expected permission mode event");
}

#[tokio::test]
async fn app_server_local_bridge_passive_event_pump_forwards_handler_events() {
    let state = Arc::new(SdkServerState::default());
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let session_id =
        coco_types::SessionId::try_new("sess-local-event-pump").expect("valid session id");
    bridge
        .install_session_snapshot(session_id.clone(), ".", "test-model")
        .await;
    let (event_tx, mut event_rx) = mpsc::channel(8);
    bridge
        .start_passive_event_pump(session_id, event_tx)
        .expect("start event pump");

    bridge
        .client()
        .set_permission_mode(
            bridge.handler(),
            coco_types::SetPermissionModeParams {
                mode: coco_types::PermissionMode::Plan,
            },
        )
        .await
        .expect("set permission mode");

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
        .await
        .expect("event delivered")
        .expect("event channel open");
    assert!(matches!(
        event,
        CoreEvent::Protocol(ServerNotification::PermissionModeChanged(params))
            if params.mode == coco_types::PermissionMode::Plan
    ));
}

#[tokio::test]
async fn app_server_local_bridge_waits_for_matching_turn_end() {
    let state = Arc::new(SdkServerState::default());
    state.install_turn_runner(Arc::new(EndingTurnRunner)).await;
    let mut bridge = AppServerLocalBridge::with_channel_capacity(Arc::clone(&state), 8);
    let session_id =
        coco_types::SessionId::try_new("sess-local-turn-wait").expect("valid session id");
    bridge
        .install_session_snapshot(session_id.clone(), ".", "test-model")
        .await;

    let completion = bridge
        .start_turn_and_wait_for_end(
            session_id,
            coco_types::TurnStartParams {
                prompt: "hello".into(),
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
            },
        )
        .await
        .expect("turn completes");

    assert_eq!(completion.ended.turn_id, completion.started.turn_id);
}

#[tokio::test]
async fn local_outbound_forwarder_routes_core_events_through_app_server() {
    let state = Arc::new(SdkServerState::default());
    let session_id =
        coco_types::SessionId::try_new("sess-local-forwarder").expect("valid session id");
    state
        .install_test_session_state(
            session_id.clone(),
            SessionMetadata {
                cwd: ".".to_string(),
                model: "test-model".to_string(),
            },
        )
        .await;

    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let surface = connection
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach surface");

    let (outbound_tx, outbound_rx) = mpsc::channel(8);
    let forwarder = spawn_app_server_local_outbound_forwarder(
        Arc::clone(&server),
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(None)),
    );

    outbound_tx
        .send(OutboundMessage::core_event(CoreEvent::Protocol(
            ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            },
        )))
        .await
        .expect("send outbound event");
    drop(outbound_tx);

    let delivered = connection.events_mut().recv().await.expect("delivery");
    assert_eq!(delivered.surface_id, surface.surface_id);
    assert_eq!(delivered.envelope.session_id, session_id);
    assert_eq!(delivered.envelope.session_seq, Some(1));
    assert!(matches!(
        delivered.envelope.event,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running
        })
    ));

    forwarder.await.expect("forwarder task");
}

#[tokio::test]
async fn app_server_bridge_runs_json_rpc_adapter_over_sdk_transport() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(11),
            method: "control/keepAlive".to_string(),
            params: serde_json::json!({}),
        }))
        .await
        .expect("send request");
    let reply = client_transport
        .recv()
        .await
        .expect("recv reply")
        .expect("reply");

    let JsonRpcMessage::Response(response) = reply else {
        panic!("expected response");
    };
    assert_eq!(response.request_id, RequestId::Integer(11));
    assert!(response.result.is_null());

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_syncs_json_rpc_session_lifecycle_to_registry() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let state = Arc::new(SdkServerState::default());
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::clone(&state),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(21),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": ".",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(21)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");
    assert_eq!(server.list_live_sessions()[0].session_id, session_id);
    assert!(state.session_handoff_snapshot(&session_id).is_some());

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(22),
            method: "session/archive".to_string(),
            params: serde_json::json!({
                "session_id": session_id,
            }),
        }))
        .await
        .expect("send session/archive");
    let _archive_response = recv_response_with_id(&client_transport, RequestId::Integer(22)).await;
    assert!(server.list_live_sessions().is_empty());

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_no_replacement_resume_installs_scoped_state_without_sdk_slot() {
    let tmp = tempfile::tempdir().expect("temp session dir");
    let manager = Arc::new(coco_session::SessionManager::new(tmp.path().to_path_buf()));
    let session_id = SessionId::try_new("bridge-scoped-resume").expect("valid session id");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).expect("create project dir");
    manager
        .save(&coco_session::Session {
            id: session_id.to_string(),
            created_at: "2026-01-15T10:00:00Z".to_string(),
            updated_at: None,
            model: "test-model".to_string(),
            working_dir: cwd.clone(),
            title: None,
            message_count: 1,
            total_tokens: 0,
            tags: Vec::new(),
        })
        .expect("save session record");
    append_bridge_transcript_message(&manager, &session_id, &cwd, 1, "user", "resume me");

    let state = Arc::new(SdkServerState::default());
    {
        state.install_session_manager(Arc::clone(&manager)).await;
    }
    let app_server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), 8);
    let client = ServerClient::connect_local(&adapter);
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let handler =
        AppServerSdkHandler::with_local_app_server(Arc::clone(&state), notif_tx, app_server);

    let resumed = client
        .session_resume(
            &handler,
            coco_types::SessionResumeParams {
                session_id: session_id.clone(),
            },
        )
        .await
        .expect("session/resume succeeds");

    assert_eq!(resumed.session.session_id, session_id);
    assert!(resumed.surface_id.is_some());
    let handoff = state
        .session_handoff_snapshot(&session_id)
        .expect("scoped resume handoff");
    assert_eq!(handoff.history.lock().await.len(), 1);
}

#[tokio::test]
async fn app_server_bridge_lists_unpersisted_live_session() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(23),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": "/tmp/live-list",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(23)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(24),
            method: "session/list".to_string(),
            params: serde_json::json!({}),
        }))
        .await
        .expect("send session/list");
    let list_response = recv_response_with_id(&client_transport, RequestId::Integer(24)).await;
    let listed: coco_types::SessionListResult =
        serde_json::from_value(list_response.result).expect("decode list result");

    let live = listed
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("live session appears in session/list");
    assert_eq!(live.cwd, "/tmp/live-list");
    assert_eq!(live.model, "test-model");

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_reads_unpersisted_live_session() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(25),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": "/tmp/live-read",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(25)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(26),
            method: "session/read".to_string(),
            params: serde_json::json!({
                "session_id": session_id,
            }),
        }))
        .await
        .expect("send session/read");
    let read_response = recv_response_with_id(&client_transport, RequestId::Integer(26)).await;
    let read: coco_types::SessionReadResult =
        serde_json::from_value(read_response.result).expect("decode read result");

    assert_eq!(read.session.cwd, "/tmp/live-read");
    assert_eq!(read.session.model, "test-model");
    assert!(read.messages.is_empty());
    assert!(!read.has_more);

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_lists_turns_for_unpersisted_live_session() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(27),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": "/tmp/live-turns",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(27)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(28),
            method: "session/turns/list".to_string(),
            params: serde_json::json!({
                "session_id": session_id,
            }),
        }))
        .await
        .expect("send session/turns/list");
    let turns_response = recv_response_with_id(&client_transport, RequestId::Integer(28)).await;
    let turns: coco_types::SessionTurnsListResult =
        serde_json::from_value(turns_response.result).expect("decode turns result");

    assert_eq!(turns.session.cwd, "/tmp/live-turns");
    assert_eq!(turns.session.model, "test-model");
    assert!(turns.turns.is_empty());
    assert!(!turns.has_more);

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn live_sdk_session_data_uses_scoped_state_without_sdk_slot() {
    let state = Arc::new(SdkServerState::default());
    let session_id = SessionId::try_new("sess-scoped-live-session-data").expect("valid session id");
    let prepared = session::PreparedStartSession {
        session_id: session_id.clone(),
        cwd: "/tmp/scoped-live-data".to_string(),
        model: "test-model".to_string(),
        permission_mode: None,
        plan_mode_instructions: None,
    };
    session::install_scoped_started_session_state(&state, &prepared, None).await;
    assert!(state.session_handoff_snapshot(&session_id).is_some());

    let (summary, history) = live_sdk_session_summary_and_history(&state, &session_id)
        .await
        .expect("scoped live state");
    assert_eq!(summary.session_id, session_id);
    assert_eq!(summary.cwd, "/tmp/scoped-live-data");
    assert_eq!(summary.model, "test-model");
    assert!(history.is_empty());
}

#[tokio::test]
async fn app_server_local_session_data_view_reads_persisted_transcript() {
    let tmp = tempfile::tempdir().expect("temp session dir");
    let manager = Arc::new(coco_session::SessionManager::new(tmp.path().to_path_buf()));
    let state = Arc::new(SdkServerState::default());
    {
        state.install_session_manager(Arc::clone(&manager)).await;
    }

    let session_id = SessionId::try_new("bridge-persisted-read").expect("valid session id");
    let cwd = Path::new("/tmp/bridge-persisted-read");
    append_bridge_transcript_message(&manager, &session_id, cwd, 1, "user", "first");
    append_bridge_transcript_message(&manager, &session_id, cwd, 2, "assistant", "reply");
    append_bridge_transcript_message(&manager, &session_id, cwd, 3, "user", "second");

    let view = LocalSessionDataView {
        app_server: Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8)),
        state,
    };

    let read_value = view
        .handle(&LocalSessionDataRequest::Read(
            coco_types::SessionReadParams {
                session_id: session_id.clone(),
                cursor: Some("1".to_string()),
                limit: Some(1),
            },
        ))
        .await
        .expect("read persisted session");
    let read: coco_types::SessionReadResult =
        serde_json::from_value(read_value).expect("decode read result");
    assert_eq!(read.session.session_id, session_id);
    assert_eq!(read.session.cwd, cwd.to_string_lossy().as_ref());
    assert_eq!(read.session.message_count, 3);
    assert_eq!(read.messages.len(), 1);
    assert_eq!(read.messages[0]["type"], "assistant");
    assert_eq!(read.messages[0]["message"]["content"], "reply");
    assert_eq!(read.next_cursor.as_deref(), Some("2"));
    assert!(read.has_more);

    let turns_value = view
        .handle(&LocalSessionDataRequest::TurnsList(
            coco_types::SessionTurnsListParams {
                session_id: session_id.clone(),
                cursor: None,
                limit: Some(1),
            },
        ))
        .await
        .expect("list persisted turns");
    let turns: coco_types::SessionTurnsListResult =
        serde_json::from_value(turns_value).expect("decode turns result");
    assert_eq!(turns.session.session_id, session_id);
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].index, 0);
    assert_eq!(turns.turns[0].start_cursor, "0");
    assert_eq!(turns.turns[0].message_count, 2);
    assert_eq!(
        turns.turns[0].started_at.as_deref(),
        Some("2026-01-15T10:01:00Z")
    );
    assert_eq!(
        turns.turns[0].ended_at.as_deref(),
        Some("2026-01-15T10:02:00Z")
    );
    assert_eq!(turns.next_cursor.as_deref(), Some("1"));
    assert!(turns.has_more);
}

#[tokio::test]
async fn app_server_bridge_subscribes_passive_surface_with_replay() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(31),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": ".",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(31)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");

    server.route_envelope(SessionEnvelope::durable(
        session_id.clone(),
        None,
        None,
        1,
        CoreEvent::Protocol(ServerNotification::SessionStateChanged {
            state: SessionState::Running,
        }),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(32),
            method: "session/subscribe".to_string(),
            params: serde_json::json!({
                "session_id": session_id,
                "after_seq": 0,
            }),
        }))
        .await
        .expect("send session/subscribe");
    let subscribe_response = recv_response_with_id(&client_transport, RequestId::Integer(32)).await;
    let subscribed: coco_types::SessionSubscribeResult =
        serde_json::from_value(subscribe_response.result).expect("decode subscribe result");

    assert_eq!(subscribed.session_id, session_id);
    assert_eq!(subscribed.replayed.len(), 1);
    assert_eq!(subscribed.replayed[0].session_seq, Some(1));
    assert_eq!(server.list_live_sessions()[0].surface_counts.attached, 2);

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_subscribe_requires_snapshot_cursor() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::new(SdkServerState::default()),
    ));

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(41),
            method: "session/start".to_string(),
            params: serde_json::json!({
                "cwd": ".",
                "model": "test-model",
            }),
        }))
        .await
        .expect("send session/start");
    let start_response = recv_response_with_id(&client_transport, RequestId::Integer(41)).await;
    let session_id: SessionId = serde_json::from_value(start_response.result["session_id"].clone())
        .expect("decode started session id");

    client_transport
        .send(JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: RequestId::Integer(42),
            method: "session/subscribe".to_string(),
            params: serde_json::json!({
                "session_id": session_id,
            }),
        }))
        .await
        .expect("send session/subscribe");

    let message = client_transport
        .recv()
        .await
        .expect("recv subscribe error")
        .expect("subscribe error");
    let JsonRpcMessage::Error(error) = message else {
        panic!("expected subscribe error");
    };
    assert_eq!(error.request_id, RequestId::Integer(42));
    assert_eq!(
        error.error.data.and_then(|data| data.get("kind").cloned()),
        Some(serde_json::json!("snapshot_required"))
    );
    assert_eq!(server.list_live_sessions()[0].surface_counts.attached, 1);

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_forwards_external_notifications() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let (external_tx, external_rx) = mpsc::channel(8);
    let bridge_task = tokio::spawn(
        run_app_server_sdk_state_over_sdk_transport_with_external_notifications(
            connection,
            server_transport,
            Arc::new(SdkServerState::default()),
            vec![external_rx],
        ),
    );

    external_tx
        .send(CoreEvent::Protocol(
            ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            },
        ))
        .await
        .expect("send external event");
    let message = client_transport
        .recv()
        .await
        .expect("recv notification")
        .expect("notification");

    let JsonRpcMessage::Notification(notification) = message else {
        panic!("expected notification");
    };
    assert_eq!(notification.method, "session/stateChanged");

    drop(external_tx);
    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

#[tokio::test]
async fn app_server_bridge_routes_server_request_reply_frames_to_sdk_state() {
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport_for_request: Arc<dyn SdkTransport> = server_transport.clone();
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let state = Arc::new(SdkServerState::default());
    let bridge_task = tokio::spawn(run_app_server_sdk_state_over_sdk_transport(
        connection,
        server_transport,
        Arc::clone(&state),
    ));

    wait_for_outbound_queue(&state).await;
    let state_for_request = Arc::clone(&state);
    let request_task = tokio::spawn(async move {
        state_for_request
            .send_server_request(
                &server_transport_for_request,
                "hook/callback",
                serde_json::json!({ "name": "stop" }),
            )
            .await
    });
    let JsonRpcMessage::Request(request) = client_transport
        .recv()
        .await
        .expect("recv server request")
        .expect("server request")
    else {
        panic!("expected server request");
    };
    assert_eq!(request.method, "hook/callback");

    client_transport
        .send(JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            request_id: request.request_id.clone(),
            result: serde_json::json!({ "ok": true }),
        }))
        .await
        .expect("send server-request response");

    let reply = request_task
        .await
        .expect("request task")
        .expect("server request resolved");
    let JsonRpcFrame::Success(response) = reply else {
        panic!("expected server-request success frame");
    };
    assert_eq!(
        response.id,
        crate::sdk_server::transport::json_rpc_id_from_request_id(request.request_id.clone())
    );
    assert_eq!(response.result, serde_json::json!({ "ok": true }));

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

async fn recv_response_with_id(
    transport: &InMemoryTransport,
    request_id: RequestId,
) -> JsonRpcResponse {
    loop {
        let message = transport
            .recv()
            .await
            .expect("recv message")
            .expect("message");
        if let JsonRpcMessage::Response(response) = message
            && response.request_id == request_id
        {
            return response;
        }
    }
}

async fn wait_for_outbound_queue(state: &SdkServerState) {
    for _ in 0..100 {
        if state.has_sdk_outbound_tx().await {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("outbound queue was not installed");
}

fn append_bridge_transcript_message(
    manager: &coco_session::SessionManager,
    session_id: &SessionId,
    cwd: &Path,
    ordinal: i32,
    entry_type: &str,
    content: &str,
) {
    let entry = coco_session::TranscriptEntry {
        entry_type: entry_type.to_string(),
        uuid: format!("{session_id}-{ordinal}"),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: Some(session_id.clone()),
        cwd: cwd.to_string_lossy().into_owned(),
        timestamp: format!("2026-01-15T10:{ordinal:02}:00Z"),
        version: None,
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({
            "role": entry_type,
            "content": content,
        })),
        usage: None,
        model: Some("test-model".to_string()),
        request_id: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    manager
        .store_for(cwd)
        .append_message(session_id.as_str(), &entry)
        .expect("append transcript message");
}

#[test]
fn decode_client_request_accepts_empty_params_for_unit_variant() {
    let request = decode_client_request("control/keepAlive", Some(serde_json::json!({})))
        .expect("decode keepAlive");

    assert!(matches!(request, ClientRequest::KeepAlive));
}

#[test]
fn legacy_json_rpc_message_converts_to_transport_frame() {
    let message = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        request_id: RequestId::Integer(7),
        method: "control/keepAlive".to_string(),
        params: serde_json::json!({}),
    });

    let frame = json_rpc_message_to_frame(message).expect("convert to frame");

    let JsonRpcFrame::Request(request) = frame else {
        panic!("expected request frame");
    };
    assert_eq!(request.id, JsonRpcId::Number(7));
    assert_eq!(request.method, "control/keepAlive");
    assert_eq!(request.params, Some(serde_json::json!({})));
}

#[test]
fn transport_frame_converts_to_legacy_json_rpc_message() {
    let frame = JsonRpcFrame::Error(TransportJsonRpcErrorResponse::new(
        JsonRpcId::String("req-1".to_string()),
        TransportJsonRpcErrorObject::new(
            -32602,
            "invalid params",
            Some(serde_json::json!({ "field": "session_id" })),
        ),
    ));

    let message = json_rpc_frame_to_message(frame).expect("convert to message");

    let JsonRpcMessage::Error(error) = message else {
        panic!("expected error message");
    };
    assert_eq!(error.request_id, RequestId::String("req-1".to_string()));
    assert_eq!(error.error.code, -32602);
    assert_eq!(
        error.error.data,
        Some(serde_json::json!({ "field": "session_id" }))
    );
}

#[test]
fn transport_null_id_is_rejected_for_legacy_json_rpc_message() {
    let frame = JsonRpcFrame::Success(JsonRpcSuccess::new(
        JsonRpcId::Null,
        serde_json::Value::Null,
    ));

    let error = json_rpc_frame_to_message(frame).expect_err("null id rejected");

    assert!(matches!(error, SdkJsonRpcFrameError::NullId));
}

fn hub_announce_frame(live_sessions: Vec<SessionId>) -> AnnounceFrame {
    AnnounceFrame {
        instance_id: Uuid::nil(),
        live_sessions,
        host: "host-a".to_string(),
        cwd: "/work".to_string(),
        pid: 42,
        started_at: chrono::Utc
            .timestamp_opt(1_704_067_200, 0)
            .single()
            .expect("fixed timestamp"),
        version: "0.1.0".to_string(),
        instance_kind: "interactive".to_string(),
        entrypoint: Some("coco".to_string()),
        name: Some("dev".to_string()),
    }
}

fn hub_worker_config(url: String, live_sessions: Vec<SessionId>) -> HubConnectorWorkerConfig {
    HubConnectorWorkerConfig {
        url,
        announce: hub_announce_frame(live_sessions),
        channel_capacity: 8,
        pending_capacity: 8,
        batch_max_events: 8,
        batch_max_bytes: 1_048_576,
        flush_interval: Duration::from_secs(60),
        reconnect_initial_delay: Duration::from_millis(10),
        reconnect_max_delay: Duration::from_millis(20),
    }
}

async fn spawn_collecting_hub_server() -> (String, mpsc::Receiver<BatchFrame>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(
            stream,
            |request: &http::Request<()>, mut response: http::Response<()>| {
                let protocol = request
                    .headers()
                    .get("Sec-WebSocket-Protocol")
                    .and_then(|value| value.to_str().ok());
                assert_eq!(protocol, Some(SUBPROTOCOL_V2));
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static(SUBPROTOCOL_V2),
                );
                Ok(response)
            },
        )
        .await
        .unwrap();

        while let Some(message) = socket.next().await {
            let WsMessage::Text(text) = message.unwrap() else {
                continue;
            };
            match serde_json::from_str::<HubFrame>(&text).unwrap() {
                HubFrame::Announce(_) => {
                    socket
                        .send(WsMessage::Text(
                            serde_json::to_string(&HubFrame::AnnounceAck(AnnounceAckFrame {
                                first_seen: false,
                                hub_version: "test".to_string(),
                                resume_from: HashMap::new(),
                            }))
                            .unwrap()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                HubFrame::Batch(batch) => {
                    let ack = ack_for_batch(&batch);
                    tx.send(batch).await.unwrap();
                    socket
                        .send(WsMessage::Text(
                            serde_json::to_string(&HubFrame::BatchAck(ack))
                                .unwrap()
                                .into(),
                        ))
                        .await
                        .unwrap();
                }
                _ => panic!("unexpected hub frame"),
            }
        }
    });
    (format!("ws://{addr}/v1/connect"), rx)
}

fn ack_for_batch(batch: &BatchFrame) -> BatchAckFrame {
    let mut up_to_seq = HashMap::<SessionId, i64>::new();
    for event in &batch.events {
        up_to_seq
            .entry(event.session_id.clone())
            .and_modify(|seq| *seq = (*seq).max(event.session_seq))
            .or_insert(event.session_seq);
    }
    BatchAckFrame { up_to_seq }
}

#[tokio::test]
async fn local_outbound_forwarder_enqueues_stamped_events_to_hub_connector() {
    let state = Arc::new(SdkServerState::default());
    let session_id = SessionId::try_new("sess-local-hub-egress").expect("valid session id");
    state
        .install_test_session_state(
            session_id.clone(),
            SessionMetadata {
                cwd: ".".to_string(),
                model: "test-model".to_string(),
            },
        )
        .await;

    let (hub_url, mut batches) = spawn_collecting_hub_server().await;
    let worker = HubConnectorWorker::spawn(hub_worker_config(hub_url, vec![session_id.clone()]))
        .expect("hub worker");

    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let surface = connection
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach surface");

    let (outbound_tx, outbound_rx) = mpsc::channel(8);
    let forwarder = spawn_app_server_local_outbound_forwarder(
        Arc::clone(&server),
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(Some(worker.sender()))),
    );

    outbound_tx
        .send(OutboundMessage::core_event(CoreEvent::Protocol(
            ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            },
        )))
        .await
        .expect("send outbound event");
    drop(outbound_tx);

    let delivered = connection.events_mut().recv().await.expect("delivery");
    assert_eq!(delivered.surface_id, surface.surface_id);
    assert_eq!(delivered.envelope.session_id, session_id);
    assert_eq!(delivered.envelope.session_seq, Some(1));

    forwarder.await.expect("forwarder task");
    let stats = tokio::time::timeout(Duration::from_secs(1), worker.shutdown_and_flush())
        .await
        .expect("hub worker shutdown")
        .expect("hub worker flush");
    let batch = tokio::time::timeout(Duration::from_secs(1), batches.recv())
        .await
        .expect("hub batch")
        .expect("hub batch channel open");

    assert_eq!(stats.shipped_events, 1);
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].session_id, session_id);
    assert_eq!(batch.events[0].session_seq, 1);
}

#[tokio::test]
async fn sdk_bridge_enqueues_protocol_notifications_to_hub_connector() {
    let state = Arc::new(SdkServerState::default());
    let session_id = SessionId::try_new("sess-sdk-hub-egress").expect("valid session id");
    state
        .install_test_session_state(
            session_id.clone(),
            SessionMetadata {
                cwd: ".".to_string(),
                model: "test-model".to_string(),
            },
        )
        .await;

    let (hub_url, mut batches) = spawn_collecting_hub_server().await;
    let worker = HubConnectorWorker::spawn(hub_worker_config(hub_url, vec![session_id.clone()]))
        .expect("hub worker");
    let server = Arc::new(AppServer::<LocalAppSessionHandle>::new(1, 8));
    let adapter = JsonRpcAdapter::with_channel_capacity(server, 8);
    let connection = adapter.connect();
    let (server_transport, client_transport) = InMemoryTransport::pair(8);
    let server_transport: Arc<dyn SdkTransport> = server_transport;
    let (external_tx, external_rx) = mpsc::channel(8);

    let bridge_task = tokio::spawn(
        run_app_server_sdk_state_over_sdk_transport_with_external_notifications_and_hub_connector(
            connection,
            server_transport,
            state,
            vec![external_rx],
            Some(worker.sender()),
            APP_SERVER_TURN_DRAIN_TIMEOUT,
        ),
    );

    external_tx
        .send(CoreEvent::Protocol(
            ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            },
        ))
        .await
        .expect("send external event");
    let message = client_transport
        .recv()
        .await
        .expect("recv notification")
        .expect("notification");
    let JsonRpcMessage::Notification(notification) = message else {
        panic!("expected notification");
    };
    assert_eq!(notification.method, "session/stateChanged");

    drop(external_tx);
    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");

    let stats = tokio::time::timeout(Duration::from_secs(1), worker.shutdown_and_flush())
        .await
        .expect("hub worker shutdown")
        .expect("hub worker flush");
    let batch = tokio::time::timeout(Duration::from_secs(1), batches.recv())
        .await
        .expect("hub batch")
        .expect("hub batch channel open");

    assert_eq!(stats.shipped_events, 1);
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.events[0].session_id, session_id);
    assert_eq!(batch.events[0].session_seq, 1);
}
