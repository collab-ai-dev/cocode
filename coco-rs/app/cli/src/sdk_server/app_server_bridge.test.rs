use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone;
use coco_app_server::AppServer;
use coco_app_server::AttachSurfaceOptions;
use coco_app_server::JsonRpcAdapter;
use coco_app_server::JsonRpcRequestHandler;
use coco_app_server::LocalClientAdapter;
use coco_app_server::LocalClientRequestHandler;
use coco_app_server::SurfaceRole;
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
use coco_types::JsonRpcMessage;
use coco_types::ServerNotification;
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
use crate::sdk_server::handlers::TurnHandoff;
use crate::sdk_server::handlers::TurnRunner;
use crate::sdk_server::transport::InMemoryTransport;
use crate::sdk_server::transport::SdkTransport;

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

    {
        let slot = state.session.read().await;
        let session = slot.as_ref().expect("session installed");
        assert_eq!(session.session_id, started.session_id);
        assert_eq!(session.model, "test-model");
    }

    let surface = bridge
        .client()
        .attach_interactive_session(started.session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive surface");
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
    assert_eq!(delivered.surface_id, surface.surface_id().clone());
    assert_eq!(delivered.envelope.session_id, started.session_id);
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
    let surface = bridge
        .client()
        .attach_interactive_session(started.session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach interactive surface");

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

    let lifecycle = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Some(lifecycle) = bridge.client_mut().try_next_session_lifecycle(&surface) {
                break lifecycle;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session ended lifecycle");
    assert_eq!(lifecycle.surface_id, surface.surface_id().clone());
    assert_eq!(
        lifecycle.effect.kind,
        coco_app_server::SurfaceLifecycleEffectKind::SessionEnded {
            session_id: started.session_id.clone(),
        }
    );
    assert!(state.session.read().await.is_none());
}

#[tokio::test]
async fn local_lifecycle_resume_closes_previous_live_session_before_registering_new() {
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
    })
    .expect("encode resume result");

    apply_local_lifecycle_request(
        Arc::clone(&app_server),
        LocalLifecycleRequest::Resume {
            live_before: vec![old_session_id.clone()],
        },
        &result,
    )
    .await
    .expect("apply resume lifecycle");

    let live = app_server.list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, new_session_id);
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
    })
    .expect("encode resume result");

    apply_local_lifecycle_request(
        Arc::clone(&app_server),
        LocalLifecycleRequest::Resume {
            live_before: vec![old_session_id.clone()],
        },
        &result,
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

    let slot = state.session.read().await;
    let session = slot.as_ref().expect("session installed");
    assert_eq!(session.session_id, session_id);
    assert_eq!(session.cwd, "/tmp/existing-session");
    assert_eq!(session.model, "existing-model");
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
    {
        let mut runner = state.turn_runner.write().await;
        *runner = Arc::new(EndingTurnRunner);
    }
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
    {
        let mut slot = state.session.write().await;
        *slot = Some(crate::sdk_server::handlers::SessionHandle::new(
            session_id.clone(),
            ".".to_string(),
            "test-model".to_string(),
        ));
    }

    let server = Arc::new(AppServer::<()>::new(1, 8));
    let adapter = LocalClientAdapter::with_channel_capacity(Arc::clone(&server), 8);
    let mut connection = adapter.connect();
    let surface = connection
        .attach_surface(session_id.clone(), AttachSurfaceOptions::default())
        .expect("attach surface");

    let (outbound_tx, outbound_rx) = mpsc::channel(8);
    let forwarder =
        spawn_app_server_local_outbound_forwarder(Arc::clone(&server), state, outbound_rx, None);

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
    #[derive(Debug, Clone)]
    struct TestHandle;

    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
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
async fn app_server_bridge_forwards_external_notifications() {
    #[derive(Debug, Clone)]
    struct TestHandle;

    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
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
async fn app_server_bridge_routes_legacy_server_request_replies_to_sdk_state() {
    #[derive(Debug, Clone)]
    struct TestHandle;

    let server = Arc::new(AppServer::<TestHandle>::new(1, 8));
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
    let JsonRpcMessage::Response(response) = reply else {
        panic!("expected server-request response");
    };
    assert_eq!(response.request_id, request.request_id);
    assert_eq!(response.result, serde_json::json!({ "ok": true }));

    drop(client_transport);
    bridge_task
        .await
        .expect("bridge task")
        .expect("bridge exits cleanly");
}

async fn wait_for_outbound_queue(state: &SdkServerState) {
    for _ in 0..100 {
        if state.outbound_tx.read().await.is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("outbound queue was not installed");
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

    let frame = legacy_json_rpc_message_to_frame(message).expect("convert to frame");

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

    let message = json_rpc_frame_to_legacy_message(frame).expect("convert to message");

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

    let error = json_rpc_frame_to_legacy_message(frame).expect_err("null id rejected");

    assert!(matches!(error, JsonRpcBridgeError::NullId));
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
    {
        let mut slot = state.session.write().await;
        *slot = Some(crate::sdk_server::handlers::SessionHandle::new(
            session_id.clone(),
            ".".to_string(),
            "test-model".to_string(),
        ));
    }

    let (hub_url, mut batches) = spawn_collecting_hub_server().await;
    let worker = HubConnectorWorker::spawn(hub_worker_config(hub_url, vec![session_id.clone()]))
        .expect("hub worker");

    let server = Arc::new(AppServer::<()>::new(1, 8));
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
        Some(worker.sender()),
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
    {
        let mut slot = state.session.write().await;
        *slot = Some(crate::sdk_server::handlers::SessionHandle::new(
            session_id.clone(),
            ".".to_string(),
            "test-model".to_string(),
        ));
    }

    let (hub_url, mut batches) = spawn_collecting_hub_server().await;
    let worker = HubConnectorWorker::spawn(hub_worker_config(hub_url, vec![session_id.clone()]))
        .expect("hub worker");
    let server = Arc::new(AppServer::<()>::new(1, 8));
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
