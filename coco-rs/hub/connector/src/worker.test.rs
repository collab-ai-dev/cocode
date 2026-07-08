use std::collections::HashMap;
use std::sync::Arc;

use chrono::TimeZone;
use coco_hub_protocol::AnnounceAckFrame;
use coco_hub_protocol::HubFrame;
use coco_hub_protocol::SUBPROTOCOL_V2;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::SessionId;
use coco_types::SessionStartedParams;
use futures::SinkExt;
use futures::StreamExt;
use http::HeaderValue;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::time::timeout;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use super::*;

fn session_id() -> SessionId {
    SessionId::try_new("session-1").expect("valid session id")
}

fn fixed_ts() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_704_067_200, 0)
        .single()
        .expect("fixed timestamp")
}

fn session_started(session_id: SessionId) -> ServerNotification {
    ServerNotification::SessionStarted(SessionStartedParams {
        session_id,
        protocol_version: "1.0".into(),
        cwd: "/work".into(),
        model: "claude".into(),
        provider: "anthropic".into(),
        permission_mode: "default".into(),
        tools: vec!["Read".into()],
        slash_commands: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        plugins: Vec::new(),
        api_key_source: None,
        betas: Vec::new(),
        version: "0.1.0".into(),
        output_style: None,
        fast_mode_state: None,
        lsp_active: false,
    })
}

fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
    SessionEnvelope::durable(
        session_id.clone(),
        None,
        None,
        seq,
        CoreEvent::Protocol(session_started(session_id)),
    )
}

fn ephemeral_envelope(session_id: SessionId) -> SessionEnvelope {
    SessionEnvelope::ephemeral(
        session_id,
        None,
        None,
        CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
            call_id: "call-1".into(),
            name: "Read".into(),
            input: json!({}),
        }),
    )
}

fn announce_frame() -> AnnounceFrame {
    AnnounceFrame {
        instance_id: Uuid::nil(),
        live_sessions: vec![session_id()],
        host: "host-a".to_string(),
        cwd: "/work".to_string(),
        pid: 42,
        started_at: fixed_ts(),
        version: "0.1.0".to_string(),
        instance_kind: "interactive".to_string(),
        entrypoint: Some("coco".to_string()),
        name: Some("dev".to_string()),
    }
}

fn worker_config(url: String) -> HubConnectorWorkerConfig {
    HubConnectorWorkerConfig {
        url,
        announce: announce_frame(),
        channel_capacity: 4,
        pending_capacity: 8,
        batch_max_events: 8,
        batch_max_bytes: 1_048_576,
        flush_interval: Duration::from_secs(60),
        reconnect_initial_delay: Duration::from_millis(10),
        reconnect_max_delay: Duration::from_millis(20),
    }
}

#[test]
fn try_enqueue_full_records_durable_drop_before_next_same_session_event() {
    let (tx, _rx) = tokio_mpsc::channel(1);
    let dropped = Arc::new(std::sync::Mutex::new(DroppedEventRanges::default()));
    let sender = HubConnectorSender {
        tx,
        dropped: Arc::clone(&dropped),
    };
    let session_id = session_id();

    sender
        .try_enqueue(durable_envelope(session_id.clone(), 1))
        .expect("first envelope fits");
    let error = sender
        .try_enqueue(durable_envelope(session_id.clone(), 2))
        .expect_err("second envelope records full queue");
    assert!(matches!(error, HubConnectorQueueError::Full));

    let config = worker_config("ws://127.0.0.1:1/v1/connect".to_string());
    let mut pending = VecDeque::new();
    let mut stats = HubConnectorWorkerStats::default();
    push_envelope(
        &config,
        &mut pending,
        &mut stats,
        &dropped,
        durable_envelope(session_id.clone(), 3),
    )
    .expect("push next event");

    assert_eq!(stats.dropped_durable_events, 1);
    assert_eq!(pending.len(), 2);
    let marker = pending.pop_front().expect("drop marker");
    assert_eq!(marker.session_id, session_id);
    assert_eq!(marker.session_seq, 2);
    assert!(matches!(
        marker.payload,
        coco_hub_protocol::EventPayload::EventsDropped {
            count: 1,
            since_seq: 2,
            until_seq: 2,
            ref reason,
        } if reason == "hub_connector_backlog_full"
    ));
    let next = pending.pop_front().expect("next event");
    assert_eq!(next.session_seq, 3);
}

#[test]
fn dropped_marker_flushes_after_older_ready_envelopes() {
    let (tx, mut rx) = tokio_mpsc::channel(2);
    let dropped = Arc::new(std::sync::Mutex::new(DroppedEventRanges::default()));
    let config = worker_config("ws://127.0.0.1:1/v1/connect".to_string());
    let session_id = session_id();
    tx.try_send(durable_envelope(session_id.clone(), 1))
        .expect("queue older event");
    record_dropped_envelope(&dropped, &durable_envelope(session_id.clone(), 2));

    let mut pending = VecDeque::new();
    let mut stats = HubConnectorWorkerStats::default();
    drain_ready(&config, &mut rx, &mut pending, &mut stats, &dropped).expect("drain older event");
    push_all_dropped_markers(&config, &dropped, &mut pending, &mut stats);

    assert_eq!(stats.dropped_durable_events, 1);
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].session_seq, 1);
    assert_eq!(pending[1].session_seq, 2);
    assert!(matches!(
        pending[1].payload,
        coco_hub_protocol::EventPayload::EventsDropped { .. }
    ));
}

async fn spawn_collecting_hub_server() -> (String, tokio_mpsc::Receiver<BatchFrame>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio_mpsc::channel(4);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hub_socket(stream).await;
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

async fn accept_hub_socket(
    stream: tokio::net::TcpStream,
) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
    accept_hdr_async(
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
    .unwrap()
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
async fn worker_batches_filters_and_flushes_on_shutdown() {
    let (url, mut batches) = spawn_collecting_hub_server().await;
    let worker = HubConnectorWorker::spawn(worker_config(url)).unwrap();
    let sender = worker.sender();
    let session_id = session_id();

    sender
        .enqueue(durable_envelope(session_id.clone(), 1))
        .await
        .unwrap();
    sender
        .enqueue(ephemeral_envelope(session_id.clone()))
        .await
        .unwrap();
    sender
        .enqueue(durable_envelope(session_id, 2))
        .await
        .unwrap();

    let stats = worker.shutdown_and_flush().await.unwrap();
    let batch = timeout(Duration::from_secs(1), batches.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(stats.shipped_events, 2);
    assert_eq!(stats.skipped_ephemeral_events, 1);
    assert_eq!(batch.events.len(), 2);
    assert_eq!(batch.events[0].session_seq, 1);
    assert_eq!(batch.events[1].session_seq, 2);
}

#[tokio::test]
async fn worker_splits_batches_by_serialized_byte_limit() {
    let (url, mut batches) = spawn_collecting_hub_server().await;
    let mut config = worker_config(url);
    config.batch_max_events = 8;

    let first = durable_envelope(session_id(), 1);
    let first_event = event_envelope_from_session_envelope(
        config.announce.instance_id,
        fixed_ts(),
        first.clone(),
    )
    .unwrap()
    .unwrap();
    let single_event_frame_bytes = serde_json::to_vec(&BatchFrame {
        events: vec![first_event],
    })
    .unwrap()
    .len();
    config.batch_max_bytes = single_event_frame_bytes + 1;

    let worker = HubConnectorWorker::spawn(config).unwrap();
    let sender = worker.sender();
    sender.enqueue(first).await.unwrap();
    sender
        .enqueue(durable_envelope(session_id(), 2))
        .await
        .unwrap();

    let stats = worker.shutdown_and_flush().await.unwrap();
    let first_batch = timeout(Duration::from_secs(1), batches.recv())
        .await
        .unwrap()
        .unwrap();
    let second_batch = timeout(Duration::from_secs(1), batches.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(stats.shipped_events, 2);
    assert_eq!(first_batch.events.len(), 1);
    assert_eq!(second_batch.events.len(), 1);
    assert_eq!(first_batch.events[0].session_seq, 1);
    assert_eq!(second_batch.events[0].session_seq, 2);
}

#[test]
fn retry_delay_jitter_stays_within_bounds() {
    let base = Duration::from_millis(100);
    let max = Duration::from_millis(500);

    for _ in 0..100 {
        let jittered = jittered_retry_delay(base, max);
        assert!(jittered >= Duration::from_millis(80));
        assert!(jittered <= Duration::from_millis(120));
    }

    assert!(jittered_retry_delay(Duration::from_secs(10), max) <= max);
}

#[tokio::test]
async fn worker_retries_after_failed_batch_without_dropping_pending_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let delivered = Arc::new(Mutex::new(Vec::<BatchFrame>::new()));
    let delivered_for_task = Arc::clone(&delivered);
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hub_socket(stream).await;
        ack_announce(&mut socket).await;
        let Some(Ok(WsMessage::Text(_))) = socket.next().await else {
            panic!("expected first batch");
        };
        drop(socket);

        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hub_socket(stream).await;
        ack_announce(&mut socket).await;
        while let Some(message) = socket.next().await {
            let WsMessage::Text(text) = message.unwrap() else {
                continue;
            };
            let HubFrame::Batch(batch) = serde_json::from_str::<HubFrame>(&text).unwrap() else {
                continue;
            };
            let ack = ack_for_batch(&batch);
            delivered_for_task.lock().await.push(batch);
            socket
                .send(WsMessage::Text(
                    serde_json::to_string(&HubFrame::BatchAck(ack))
                        .unwrap()
                        .into(),
                ))
                .await
                .unwrap();
        }
    });

    let mut config = worker_config(format!("ws://{addr}/v1/connect"));
    config.batch_max_events = 1;
    let worker = HubConnectorWorker::spawn(config).unwrap();
    worker
        .sender()
        .enqueue(durable_envelope(session_id(), 1))
        .await
        .unwrap();

    let stats = timeout(Duration::from_secs(2), worker.shutdown_and_flush())
        .await
        .unwrap()
        .unwrap();
    let delivered = delivered.lock().await;

    assert_eq!(stats.shipped_events, 1);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].events[0].session_seq, 1);
}

async fn ack_announce(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    while let Some(message) = socket.next().await {
        let WsMessage::Text(text) = message.unwrap() else {
            continue;
        };
        let HubFrame::Announce(_) = serde_json::from_str::<HubFrame>(&text).unwrap() else {
            continue;
        };
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
        break;
    }
}
