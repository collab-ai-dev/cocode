use chrono::TimeZone;
use chrono::Utc;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_hub_protocol::HubFrame;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_types::SessionId;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::time::Duration;
use tokio::time::timeout;
use uuid::Uuid;

use super::AppState;
use super::handle_hub_frame;
use crate::sqlite_store::SqliteEventStore;
use crate::store::EventStore;

fn instance_id() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
}

fn session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn fixed_ts(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().unwrap()
}

fn announce(live_sessions: Vec<SessionId>) -> AnnounceFrame {
    AnnounceFrame {
        instance_id: instance_id(),
        live_sessions,
        host: "host-a".to_string(),
        cwd: "/work/project".to_string(),
        pid: 42,
        started_at: fixed_ts(1_704_067_200),
        version: "0.1.0".to_string(),
        instance_kind: "interactive".to_string(),
        entrypoint: Some("coco".to_string()),
        name: Some("dev".to_string()),
    }
}

fn event(session_id: SessionId, session_seq: i64) -> EventEnvelope {
    EventEnvelope {
        instance_id: instance_id(),
        session_id: session_id.clone(),
        agent_id: None,
        session_seq,
        ts: fixed_ts(1_704_067_200 + session_seq),
        schema_version: SCHEMA_VERSION_V2,
        payload: EventPayload::Protocol {
            value: json!({
                "method": "session/started",
                "params": {
                    "session_id": session_id,
                    "cwd": "/work/project",
                    "model": "claude-test"
                }
            }),
        },
    }
}

#[tokio::test]
async fn hub_frame_handler_announces_ingests_and_resumes_per_session() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let state = AppState::new(store.clone());
    let session = session_id("session-a");
    let mut announced = None;

    let response = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Announce(announce(vec![session.clone()])),
    )
    .await;
    let HubFrame::AnnounceAck(ack) = response else {
        panic!("expected announce ack");
    };
    assert!(ack.first_seen);
    assert_eq!(ack.resume_from.get(&session), Some(&0));

    let response = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Batch(BatchFrame {
            events: vec![event(session.clone(), 7)],
        }),
    )
    .await;
    let HubFrame::BatchAck(ack) = response else {
        panic!("expected batch ack");
    };
    assert_eq!(ack.up_to_seq.get(&session), Some(&7));
    assert!(
        store
            .get_event(&instance_id().to_string(), session.as_str(), 7)
            .await
            .unwrap()
            .is_some()
    );

    let response = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Announce(announce(vec![session.clone()])),
    )
    .await;
    let HubFrame::AnnounceAck(ack) = response else {
        panic!("expected announce ack");
    };
    assert_eq!(ack.resume_from.get(&session), Some(&7));
}

#[tokio::test]
async fn hub_frame_handler_requires_announce_before_batch() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let state = AppState::new(store);
    let session = session_id("session-a");
    let mut announced = None;

    let response = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Batch(BatchFrame {
            events: vec![event(session, 1)],
        }),
    )
    .await;
    let HubFrame::Error(error) = response else {
        panic!("expected error frame");
    };
    assert_eq!(error.code, "announce_required");
}

#[tokio::test]
async fn hub_frame_handler_publishes_new_batch_events_to_live_subscribers() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let state = AppState::new(store);
    let session = session_id("session-a");
    let mut live = state.subscribe_session(instance_id().to_string(), session.clone());
    let mut announced = None;

    let _ = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Announce(announce(vec![session.clone()])),
    )
    .await;
    let response = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Batch(BatchFrame {
            events: vec![event(session.clone(), 9)],
        }),
    )
    .await;
    assert!(matches!(response, HubFrame::BatchAck(_)));

    let event = timeout(Duration::from_secs(1), live.recv())
        .await
        .expect("live event should be published")
        .expect("live channel should remain open");
    assert_eq!(event.session_id, session);
    assert_eq!(event.session_seq, 9);
}

#[tokio::test]
async fn hub_frame_handler_does_not_republish_duplicate_batch_events() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let state = AppState::new(store);
    let session = session_id("session-a");
    let mut live = state.subscribe_session(instance_id().to_string(), session.clone());
    let mut announced = None;

    let _ = handle_hub_frame(
        &state,
        &mut announced,
        HubFrame::Announce(announce(vec![session.clone()])),
    )
    .await;
    for _ in 0..2 {
        let response = handle_hub_frame(
            &state,
            &mut announced,
            HubFrame::Batch(BatchFrame {
                events: vec![event(session.clone(), 9)],
            }),
        )
        .await;
        assert!(matches!(response, HubFrame::BatchAck(_)));
    }

    let first = timeout(Duration::from_secs(1), live.recv())
        .await
        .expect("first live event should be published")
        .expect("live channel should remain open");
    assert_eq!(first.session_seq, 9);
    let duplicate = timeout(Duration::from_millis(50), live.recv()).await;
    assert!(
        duplicate.is_err(),
        "duplicate retry batch should not publish a second live event"
    );
}
