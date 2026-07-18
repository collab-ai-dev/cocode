use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;

use coco_app_server_transport::{JsonRpcFrame, JsonRpcId, JsonRpcRequest};
use coco_config::EnvKey;
use coco_hub_connector::protocol::{
    AnnounceAckFrame, AnnounceFrame, BatchAckFrame, BatchFrame, HubFrame, SUBPROTOCOL_V2,
};
use futures::{SinkExt, StreamExt};
use http::HeaderValue;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::app_server_host::request_handlers::APP_SERVER_PROTOCOL_VERSION;

use super::*;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, path: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: this test holds the crate-wide config env lock.
        unsafe { std::env::set_var(key, path.as_os_str()) };
        Self { key, previous }
    }

    fn set_str(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: this test holds the crate-wide config env lock.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => {
                // SAFETY: this test holds the crate-wide config env lock.
                unsafe { std::env::set_var(self.key, value) };
            }
            None => {
                // SAFETY: this test holds the crate-wide config env lock.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

async fn spawn_announce_hub_server() -> (String, mpsc::Receiver<AnnounceFrame>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test hub");
    let addr = listener.local_addr().expect("test hub local addr");
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept test hub client");
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
            .expect("accept hub websocket");
            while let Some(message) = socket.next().await {
                let Ok(message) = message else {
                    break;
                };
                let WsMessage::Text(text) = message else {
                    continue;
                };
                match serde_json::from_str::<HubFrame>(&text).expect("hub frame") {
                    HubFrame::Announce(announce) => {
                        if tx.send(announce).await.is_err() {
                            return;
                        }
                        if socket
                            .send(WsMessage::Text(
                                serde_json::to_string(&HubFrame::AnnounceAck(AnnounceAckFrame {
                                    first_seen: false,
                                    hub_version: "test".to_string(),
                                    resume_from: HashMap::new(),
                                }))
                                .expect("serialize announce ack")
                                .into(),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    HubFrame::Batch(batch) => {
                        if socket
                            .send(WsMessage::Text(
                                serde_json::to_string(&HubFrame::BatchAck(ack_for_batch(&batch)))
                                    .expect("serialize batch ack")
                                    .into(),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => panic!("unexpected hub frame: {other:?}"),
                }
            }
        }
    });
    (format!("ws://{addr}/v1/connect"), rx)
}

fn ack_for_batch(batch: &BatchFrame) -> BatchAckFrame {
    let mut up_to_seq = HashMap::<coco_types::SessionId, i64>::new();
    for event in &batch.events {
        up_to_seq
            .entry(event.session_id.clone())
            .and_modify(|seq| *seq = (*seq).max(event.session_seq))
            .or_insert(event.session_seq);
    }
    BatchAckFrame {
        up_to_seq,
        ..Default::default()
    }
}

async fn next_announce(announces: &mut mpsc::Receiver<AnnounceFrame>) -> AnnounceFrame {
    tokio::time::timeout(std::time::Duration::from_secs(2), announces.recv())
        .await
        .expect("event hub announce timeout")
        .expect("event hub announce")
}

fn assert_live_membership(
    announce: AnnounceFrame,
    expected: impl IntoIterator<Item = coco_types::SessionId>,
) {
    let mut actual = announce.live_sessions;
    let mut expected = expected.into_iter().collect::<Vec<_>>();
    actual.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "current_thread")]
async fn host_builder_starts_without_placeholder_session() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.lock().await;
    let config_home = tempfile::TempDir::new().expect("config home tempdir");
    let cwd = tempfile::TempDir::new().expect("cwd tempdir");
    let _guard = EnvVarGuard::set_path(coco_utils_common::COCO_CONFIG_DIR_ENV, config_home.path());

    let host = HostBuilder::new(
        RemoteHostOptions {
            agent_host_options: crate::AgentHostOptions {
                models_main: Some("anthropic/claude-opus-4-7".into()),
                ..Default::default()
            },
            max_turns: Some(1),
        },
        cwd.path().to_path_buf(),
        coco_app_runtime::ProcessRuntime::global(),
    )
    .prepare()
    .await
    .expect("prepare remote host");

    assert!(
        host.app_server.list_live_sessions().is_empty(),
        "remote host startup must not register a placeholder session"
    );

    let connection = host.connect();
    let binding = host.bridge_host().open_connection_binding(
        Arc::clone(&host.app_server),
        connection.connection_key(),
        Vec::new(),
        /*outbound_channel_capacity*/ 8,
    );
    let response = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(1),
                "initialize",
                Some(
                    serde_json::to_value(coco_types::InitializeParams::default())
                        .expect("serialize initialize params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;

    match response {
        JsonRpcFrame::Success(success) => {
            let result: coco_types::InitializeResult =
                serde_json::from_value(success.result).expect("initialize result");
            assert_eq!(result.protocol_version, APP_SERVER_PROTOCOL_VERSION);
        }
        other => panic!("initialize without live runtime should succeed, got {other:?}"),
    }

    assert!(
        host.app_server.list_live_sessions().is_empty(),
        "initialize must not create a session"
    );

    drop(binding);
    host.shutdown().await.expect("remote host shutdown");
}

#[tokio::test(flavor = "current_thread")]
async fn host_builder_updates_event_hub_membership_from_registry() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.lock().await;
    let (hub_url, mut announces) = spawn_announce_hub_server().await;
    let config_home = tempfile::TempDir::new().expect("config home tempdir");
    let cwd = tempfile::TempDir::new().expect("cwd tempdir");
    let _config_guard =
        EnvVarGuard::set_path(coco_utils_common::COCO_CONFIG_DIR_ENV, config_home.path());
    let _event_hub_guard = EnvVarGuard::set_str(EnvKey::CocoEventHubUrl.as_str(), &hub_url);

    let host = HostBuilder::new(
        RemoteHostOptions {
            agent_host_options: crate::AgentHostOptions {
                models_main: Some("anthropic/claude-opus-4-7".into()),
                ..Default::default()
            },
            max_turns: Some(1),
        },
        cwd.path().to_path_buf(),
        coco_app_runtime::ProcessRuntime::global(),
    )
    .prepare()
    .await
    .expect("prepare remote host");

    let announce = next_announce(&mut announces).await;
    assert_live_membership(announce, std::iter::empty::<coco_types::SessionId>());
    assert!(
        host.app_server.list_live_sessions().is_empty(),
        "event hub startup must not create a placeholder session"
    );

    let connection = host.connect();
    let binding = host.bridge_host().open_connection_binding(
        Arc::clone(&host.app_server),
        connection.connection_key(),
        Vec::new(),
        /*outbound_channel_capacity*/ 8,
    );
    let initialize = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(1),
                "initialize",
                Some(
                    serde_json::to_value(coco_types::InitializeParams::default())
                        .expect("serialize initialize params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;
    if !matches!(initialize, JsonRpcFrame::Success(_)) {
        panic!("initialize should succeed before session/start, got {initialize:?}");
    }

    let first_start = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(2),
                "session/start",
                Some(
                    serde_json::to_value(coco_types::SessionStartParams::default())
                        .expect("serialize session/start params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;
    let first = match first_start {
        JsonRpcFrame::Success(success) => {
            serde_json::from_value::<coco_types::SessionStartResult>(success.result)
                .expect("session/start result")
        }
        other => panic!("session/start should succeed, got {other:?}"),
    };

    assert_live_membership(
        next_announce(&mut announces).await,
        [first.session_id.clone()],
    );

    let second_start = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(3),
                "session/start",
                Some(
                    serde_json::to_value(coco_types::SessionStartParams::default())
                        .expect("serialize second session/start params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;
    let second = match second_start {
        JsonRpcFrame::Success(success) => {
            serde_json::from_value::<coco_types::SessionStartResult>(success.result)
                .expect("second session/start result")
        }
        other => panic!("second session/start should succeed, got {other:?}"),
    };

    assert_live_membership(
        next_announce(&mut announces).await,
        [first.session_id.clone(), second.session_id.clone()],
    );

    drop(binding);
    tokio::time::timeout(std::time::Duration::from_secs(10), host.shutdown())
        .await
        .expect("remote host shutdown timeout")
        .expect("remote host shutdown");
}

#[tokio::test(flavor = "current_thread")]
async fn max_sessions_one_allows_first_real_session_without_placeholder() {
    let _lock = crate::test_support::CONFIG_ENV_LOCK.lock().await;
    let config_home = tempfile::TempDir::new().expect("config home tempdir");
    let cwd = tempfile::TempDir::new().expect("cwd tempdir");
    let _config_guard =
        EnvVarGuard::set_path(coco_utils_common::COCO_CONFIG_DIR_ENV, config_home.path());
    let _max_sessions_guard = EnvVarGuard::set_str(EnvKey::CocoServerMaxSessions.as_str(), "1");

    let host = HostBuilder::new(
        RemoteHostOptions {
            agent_host_options: crate::AgentHostOptions {
                models_main: Some("anthropic/claude-opus-4-7".into()),
                ..Default::default()
            },
            max_turns: Some(1),
        },
        cwd.path().to_path_buf(),
        coco_app_runtime::ProcessRuntime::global(),
    )
    .prepare()
    .await
    .expect("prepare remote host");

    assert!(
        host.app_server.list_live_sessions().is_empty(),
        "remote host startup must not consume the only max_sessions slot"
    );

    let connection = host.connect();
    let binding = host.bridge_host().open_connection_binding(
        Arc::clone(&host.app_server),
        connection.connection_key(),
        Vec::new(),
        /*outbound_channel_capacity*/ 8,
    );

    let initialize = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(1),
                "initialize",
                Some(
                    serde_json::to_value(coco_types::InitializeParams::default())
                        .expect("serialize initialize params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;
    match initialize {
        JsonRpcFrame::Success(success) => {
            let result: coco_types::InitializeResult =
                serde_json::from_value(success.result).expect("initialize result");
            assert_eq!(result.protocol_version, APP_SERVER_PROTOCOL_VERSION);
        }
        other => panic!("initialize should succeed before session/start, got {other:?}"),
    }

    let start = connection
        .dispatch_client_request(
            JsonRpcRequest::new(
                JsonRpcId::Number(2),
                "session/start",
                Some(
                    serde_json::to_value(coco_types::SessionStartParams::default())
                        .expect("serialize session/start params"),
                ),
            ),
            binding.handler.as_ref(),
        )
        .await;

    let result = match start {
        JsonRpcFrame::Success(success) => {
            serde_json::from_value::<coco_types::SessionStartResult>(success.result)
                .expect("session/start result")
        }
        other => panic!(
            "first real session/start must succeed with COCO_SERVER_MAX_SESSIONS=1, got {other:?}"
        ),
    };
    // session/start always attaches the caller surface; `surface_id` is required
    // on the result, so a successful start proves the attachment.
    let _ = &result.surface_id;

    let live_sessions = host.app_server.list_live_sessions();
    assert_eq!(live_sessions.len(), 1);
    assert_eq!(live_sessions[0].session_id, result.session_id);

    drop(binding);
    host.shutdown().await.expect("remote host shutdown");
}
