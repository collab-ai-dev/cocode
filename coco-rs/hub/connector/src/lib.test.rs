use std::collections::HashMap;

use chrono::TimeZone;
use coco_hub_protocol::AnnounceAckFrame;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_types::AgentId;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::EventReplayPolicy;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionStartedParams;
use futures::SinkExt;
use futures::StreamExt;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use super::*;

fn session_id() -> SessionId {
    SessionId::try_new("session-1").expect("valid session id")
}

fn agent_id() -> AgentId {
    AgentId::try_new_generated("aagent-0000000000000001").expect("valid generated agent id")
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

fn announce_frame(live_sessions: Vec<SessionId>) -> AnnounceFrame {
    AnnounceFrame {
        instance_id: Uuid::nil(),
        live_sessions,
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

async fn spawn_hub_test_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
            let frame = serde_json::from_str::<HubFrame>(&text).unwrap();
            let response = match frame {
                HubFrame::Announce(announce) => {
                    let resume_from = announce
                        .live_sessions
                        .into_iter()
                        .map(|session_id| (session_id, 3))
                        .collect::<HashMap<_, _>>();
                    HubFrame::AnnounceAck(AnnounceAckFrame {
                        first_seen: false,
                        hub_version: "test".to_string(),
                        resume_from,
                    })
                }
                HubFrame::Batch(batch) => {
                    let mut up_to_seq = HashMap::<SessionId, i64>::new();
                    for event in batch.events {
                        up_to_seq
                            .entry(event.session_id)
                            .and_modify(|seq| *seq = (*seq).max(event.session_seq))
                            .or_insert(event.session_seq);
                    }
                    HubFrame::BatchAck(BatchAckFrame {
                        up_to_seq,
                        ..Default::default()
                    })
                }
                _ => panic!("unexpected client frame"),
            };
            socket
                .send(WsMessage::Text(
                    serde_json::to_string(&response).unwrap().into(),
                ))
                .await
                .unwrap();
        }
    });
    format!("ws://{addr}/v1/connect")
}

#[test]
fn durable_protocol_envelope_becomes_hub_event_envelope() {
    let session_id = session_id();
    let agent_id = agent_id();
    let envelope = SessionEnvelope::durable(
        session_id.clone(),
        Some(agent_id.clone()),
        None,
        42,
        CoreEvent::Protocol(session_started(session_id.clone())),
    );

    let event = event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope).unwrap();
    let event = event.expect("durable envelope should be shipped");

    assert_eq!(event.instance_id, Uuid::nil());
    assert_eq!(event.session_id, session_id);
    assert_eq!(event.agent_id, Some(agent_id));
    assert_eq!(event.session_seq, 42);
    assert_eq!(event.schema_version, SCHEMA_VERSION_V2);

    let EventPayload::Protocol { value } = event.payload else {
        panic!("expected protocol payload");
    };
    assert_eq!(value["method"], "session/started");
    assert_eq!(value["params"]["session_id"], "session-1");
}

#[test]
fn batch_conversion_preserves_durable_order_and_skips_ephemeral() {
    let first_session = session_id();
    let second_session = SessionId::try_new("session-2").expect("valid session id");
    let envelopes = vec![
        SessionEnvelope::durable(
            first_session.clone(),
            None,
            None,
            1,
            CoreEvent::Protocol(session_started(first_session.clone())),
        ),
        SessionEnvelope::ephemeral(
            first_session.clone(),
            None,
            None,
            CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
                call_id: "call-1".into(),
                name: "Read".into(),
                input: json!({}),
            }),
        ),
        SessionEnvelope::durable(
            second_session.clone(),
            None,
            None,
            7,
            CoreEvent::Protocol(session_started(second_session.clone())),
        ),
    ];

    let batch = batch_frame_from_session_envelopes(Uuid::nil(), fixed_ts, envelopes).unwrap();

    assert_eq!(batch.events.len(), 2);
    assert_eq!(batch.events[0].session_id, first_session);
    assert_eq!(batch.events[0].session_seq, 1);
    assert_eq!(batch.events[1].session_id, second_session);
    assert_eq!(batch.events[1].session_seq, 7);
}

#[test]
fn ephemeral_envelope_is_not_shipped_to_hub() {
    let envelope = SessionEnvelope::stamp(
        session_id(),
        None,
        CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
            call_id: "call-1".into(),
            name: "Read".into(),
            input: json!({"file_path": "a.txt"}),
        }),
        || panic!("ephemeral events must not allocate session_seq"),
    );

    assert_eq!(envelope.event.replay_policy(), EventReplayPolicy::Ephemeral);
    let event = event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope).unwrap();
    assert!(event.is_none());
}

#[test]
fn sequenced_non_protocol_envelope_is_rejected() {
    let envelope = SessionEnvelope::durable(
        session_id(),
        None,
        None,
        1,
        CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
            call_id: "call-1".into(),
            name: "Read".into(),
            input: json!({}),
        }),
    );

    let err = event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope)
        .expect_err("sequenced stream event must be rejected");
    assert!(matches!(err, EnvelopeEgressError::NonProtocolDurable));
}

#[tokio::test]
async fn connector_client_announces_and_sends_batches_over_websocket() {
    let url = spawn_hub_test_server().await;
    let session_id = session_id();
    let mut client = HubConnectorClient::connect(&url).await.unwrap();

    let announce_ack = client
        .announce(announce_frame(vec![session_id.clone()]))
        .await
        .unwrap();
    assert_eq!(announce_ack.resume_from.get(&session_id), Some(&3));

    let batch_ack = client
        .send_batch(BatchFrame {
            events: vec![
                event_envelope_from_session_envelope(
                    Uuid::nil(),
                    fixed_ts(),
                    SessionEnvelope::durable(
                        session_id.clone(),
                        None,
                        None,
                        4,
                        CoreEvent::Protocol(session_started(session_id.clone())),
                    ),
                )
                .unwrap()
                .unwrap(),
                event_envelope_from_session_envelope(
                    Uuid::nil(),
                    fixed_ts(),
                    SessionEnvelope::durable(
                        session_id.clone(),
                        None,
                        None,
                        8,
                        CoreEvent::Protocol(session_started(session_id.clone())),
                    ),
                )
                .unwrap()
                .unwrap(),
            ],
        })
        .await
        .unwrap();
    assert_eq!(batch_ack.up_to_seq.get(&session_id), Some(&8));
}
