//! Wire-format guards for the Event Hub protocol.
//!
//! This crate is shared by the agent-side connector and the hub server, so its
//! JSON shape is a contract. These tests pin it three ways:
//!   1. round-trip every `HubFrame` variant (serialize → deserialize → eq),
//!   2. golden-snapshot the on-wire JSON of the representative frames (locks
//!      `kind` tags + camelCase field renames),
//!   3. a constants guard so a `SCHEMA_VERSION` / subprotocol bump is a
//!      conscious, reviewed change.
//!
//! Fixed UUID/timestamp keep snapshots stable across runs and machines.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;
use coco_hub_protocol::AnnounceAckFrame;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::ErrorFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_hub_protocol::HubFrame;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_hub_protocol::SUBPROTOCOL_V2;
use coco_types::AgentId;
use coco_types::SessionId;
use serde_json::json;
use uuid::Uuid;

fn fixed_ts() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_704_067_200, 0).expect("fixed timestamp")
}

fn announce() -> AnnounceFrame {
    AnnounceFrame {
        instance_id: Uuid::nil(),
        live_sessions: vec![session_id()],
        host: "test-host".to_string(),
        cwd: "/work".to_string(),
        pid: 4242,
        started_at: fixed_ts(),
        version: "0.1.0".to_string(),
        instance_kind: "cli".to_string(),
        entrypoint: Some("coco".to_string()),
        name: Some("demo".to_string()),
    }
}

fn session_id() -> SessionId {
    SessionId::try_new("sess-1").expect("valid session id")
}

fn agent_id() -> AgentId {
    AgentId::try_new_generated("aagent-0000000000000001").expect("valid generated agent id")
}

fn cursor_map(seq: i64) -> HashMap<SessionId, i64> {
    HashMap::from([(session_id(), seq)])
}

fn envelope(payload: EventPayload) -> EventEnvelope {
    EventEnvelope {
        instance_id: Uuid::nil(),
        session_id: session_id(),
        agent_id: Some(agent_id()),
        session_seq: 7,
        ts: fixed_ts(),
        schema_version: SCHEMA_VERSION_V2,
        payload,
    }
}

/// One representative of each `HubFrame` variant.
fn all_hub_frames() -> Vec<HubFrame> {
    vec![
        HubFrame::Announce(announce()),
        HubFrame::AnnounceAck(AnnounceAckFrame {
            first_seen: true,
            hub_version: "0.1.0".to_string(),
            resume_from: cursor_map(42),
        }),
        HubFrame::Batch(BatchFrame {
            events: vec![envelope(EventPayload::Protocol {
                value: json!({"method": "turn/started"}),
            })],
        }),
        HubFrame::BatchAck(BatchAckFrame {
            up_to_seq: cursor_map(9),
            rejected: Default::default(),
        }),
        HubFrame::Error(ErrorFrame {
            code: "rate_limited".to_string(),
            detail: "slow down".to_string(),
        }),
    ]
}

/// Every `EventPayload` variant, paired with its expected `kind` tag.
fn all_event_payloads() -> Vec<EventPayload> {
    vec![
        EventPayload::Protocol { value: json!({}) },
        EventPayload::ToolUseQueued { value: json!({}) },
        EventPayload::ToolUseStarted { value: json!({}) },
        EventPayload::ToolUseCompleted { value: json!({}) },
        EventPayload::McpToolCallBegin { value: json!({}) },
        EventPayload::McpToolCallEnd { value: json!({}) },
        EventPayload::TextBlockCompleted { value: json!({}) },
        EventPayload::ThinkingBlockCompleted { value: json!({}) },
        EventPayload::EventsDropped {
            count: 3,
            since_seq: 10,
            until_seq: 13,
            reason: "backpressure".to_string(),
        },
        EventPayload::Unknown { value: json!({}) },
    ]
}

#[test]
fn roundtrip_all_hubframe_variants() {
    for frame in all_hub_frames() {
        let json = serde_json::to_string(&frame).unwrap();
        let back: HubFrame = serde_json::from_str(&json).unwrap();
        pretty_assertions::assert_eq!(frame, back, "round-trip failed for {json}");
    }
}

#[test]
fn roundtrip_all_event_payloads() {
    for payload in all_event_payloads() {
        let env = envelope(payload);
        let json = serde_json::to_string(&env).unwrap();
        let back: EventEnvelope = serde_json::from_str(&json).unwrap();
        pretty_assertions::assert_eq!(env, back, "round-trip failed for {json}");
    }
}

#[test]
fn snapshot_announce_frame_json() {
    // Locks the `announce` tag + every camelCase field rename.
    let json = serde_json::to_string_pretty(&HubFrame::Announce(announce())).unwrap();
    insta::assert_snapshot!("hubframe_announce", json);
}

#[test]
fn snapshot_cursor_ack_frames_json() {
    // Locks the v2 per-session cursor maps that replaced the old single
    // process-wide cursor fields.
    let announce_ack = HubFrame::AnnounceAck(AnnounceAckFrame {
        first_seen: true,
        hub_version: "0.1.0".to_string(),
        resume_from: cursor_map(42),
    });
    let batch_ack = HubFrame::BatchAck(BatchAckFrame {
        up_to_seq: cursor_map(9),
        rejected: Default::default(),
    });
    let json = serde_json::to_string_pretty(&vec![announce_ack, batch_ack]).unwrap();
    insta::assert_snapshot!("hubframe_cursor_acks", json);
}

#[test]
fn snapshot_batch_with_event_json() {
    // Locks the nested EventEnvelope camelCase shape + payload `kind` tagging.
    let frame = HubFrame::Batch(BatchFrame {
        events: vec![envelope(EventPayload::ToolUseCompleted {
            value: json!({"call_id": "c1", "is_error": false}),
        })],
    });
    let json = serde_json::to_string_pretty(&frame).unwrap();
    insta::assert_snapshot!("hubframe_batch_with_event", json);
}

#[test]
fn snapshot_event_payload_kind_tags() {
    // One stable snapshot of every payload's `kind` discriminant — a rename of
    // any variant tag shows up here without a test per variant.
    let kinds: Vec<String> = all_event_payloads()
        .iter()
        .map(|p| {
            let value = serde_json::to_value(p).unwrap();
            value
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("<missing>")
                .to_string()
        })
        .collect();
    insta::assert_snapshot!("event_payload_kind_tags", kinds.join("\n"));
}

#[test]
fn snapshot_events_dropped_payload_json() {
    let payload = EventPayload::EventsDropped {
        count: 3,
        since_seq: 10,
        until_seq: 13,
        reason: "backpressure".to_string(),
    };
    let json = serde_json::to_string_pretty(&payload).unwrap();
    insta::assert_snapshot!("event_payload_events_dropped", json);
}

#[test]
fn wire_constants_guard() {
    // A bump here must be a conscious, reviewed change — not an accident.
    assert_eq!(SCHEMA_VERSION_V2, 2);
    assert_eq!(SUBPROTOCOL_V2, "coco-event-hub.v2");
}
