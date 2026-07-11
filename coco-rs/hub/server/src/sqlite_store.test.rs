use chrono::TimeZone;
use chrono::Utc;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_types::AgentId;
use coco_types::SessionId;
use pretty_assertions::assert_eq;
use serde_json::json;
use uuid::Uuid;

use super::SqliteEventStore;
use crate::store::EventFilter;
use crate::store::EventQuery;
use crate::store::EventStore;
use crate::store::ListInstancesParams;
use crate::store::ListSessionsParams;
use crate::store::RetentionPolicy;
use crate::store::SearchQuery;

fn session_id(value: &str) -> SessionId {
    SessionId::try_new(value).expect("valid session id")
}

fn agent_id() -> AgentId {
    AgentId::try_new_generated("aagent-0000000000000001").expect("valid agent id")
}

fn instance_id() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
}

fn instance_id_b() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap()
}

fn announce_for(instance_id: Uuid, live_sessions: Vec<SessionId>) -> AnnounceFrame {
    AnnounceFrame {
        instance_id,
        ..announce(live_sessions)
    }
}

fn protocol_event_for(instance_id: Uuid, session_id: SessionId, session_seq: i64) -> EventEnvelope {
    EventEnvelope {
        instance_id,
        ..protocol_event(session_id, session_seq)
    }
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

fn protocol_event(session_id: SessionId, session_seq: i64) -> EventEnvelope {
    EventEnvelope {
        instance_id: instance_id(),
        session_id: session_id.clone(),
        agent_id: Some(agent_id()),
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

fn tool_event(session_id: SessionId, session_seq: i64) -> EventEnvelope {
    EventEnvelope {
        instance_id: instance_id(),
        session_id,
        agent_id: None,
        session_seq,
        ts: fixed_ts(1_704_067_200 + session_seq),
        schema_version: SCHEMA_VERSION_V2,
        payload: EventPayload::ToolUseQueued {
            value: json!({
                "turn_id": "t-0000000000000001",
                "call_id": "call-1",
                "name": "Read",
                "input": {"file_path": "README.md"}
            }),
        },
    }
}

#[tokio::test]
async fn sqlite_store_upserts_instances_sessions_and_reports_ingest_health() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let session = session_id("session-a");

    let outcome = store
        .upsert_instance(&announce(vec![session.clone()]))
        .await
        .unwrap();
    assert!(outcome.first_seen);

    let health = store.health().await.unwrap();
    assert!(health.ingest_supported);
    assert!(!health.read_only);

    let instances = store
        .list_instances(ListInstancesParams::default())
        .await
        .unwrap();
    assert_eq!(instances.items.len(), 1);
    assert_eq!(instances.items[0].session_count, 1);

    let sessions = store
        .list_sessions(&instance_id().to_string(), ListSessionsParams::default())
        .await
        .unwrap();
    assert_eq!(sessions.items.len(), 1);
    assert_eq!(sessions.items[0].session_id, session);
    assert_eq!(sessions.items[0].discovered_via, "announce");
}

#[tokio::test]
async fn sqlite_store_ingests_events_with_session_seq_deduplication() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let session = session_id("session-a");
    store
        .upsert_instance(&announce(vec![session.clone()]))
        .await
        .unwrap();

    let stats = store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![
                    protocol_event(session.clone(), 1),
                    tool_event(session.clone(), 2),
                    tool_event(session.clone(), 2),
                ],
            },
        )
        .await
        .unwrap();

    assert_eq!(stats.accepted, 2);
    assert_eq!(stats.duplicates, 1);
    assert_eq!(stats.parse_failures, 0);
    assert_eq!(stats.rejected_conflicts, 0);

    let events = store
        .list_events(EventQuery {
            instance_id: instance_id().to_string(),
            session_id: Some(session.clone()),
            before: None,
            limit: 100,
            filter: EventFilter::default(),
        })
        .await
        .unwrap();
    assert_eq!(
        events
            .items
            .iter()
            .map(|event| event.session_seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(events.items[1].tool_name.as_deref(), Some("Read"));

    let session_row = store
        .get_session(&instance_id().to_string(), session.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session_row.last_seq, 2);
    assert_eq!(session_row.model.as_deref(), Some("claude-test"));
    assert_eq!(session_row.message_count, 2);
}

#[tokio::test]
async fn sqlite_store_rejects_session_seq_regression_without_overwriting() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let session = session_id("session-a");
    store
        .upsert_instance(&announce(vec![session.clone()]))
        .await
        .unwrap();

    // Original event at seq 1.
    let first = store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![tool_event(session.clone(), 1)],
            },
        )
        .await
        .unwrap();
    assert_eq!(first.accepted, 1);
    assert_eq!(first.rejected_conflicts, 0);

    // A *different* event re-uses seq 1 (a regression a restarted process
    // would emit without skip-ahead). It must be rejected, not stored.
    let regression = store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![protocol_event(session.clone(), 1)],
            },
        )
        .await
        .unwrap();
    assert_eq!(regression.accepted, 0);
    assert_eq!(regression.duplicates, 0);
    assert_eq!(regression.rejected_conflicts, 1);

    // A byte-identical retry of the original is still a benign duplicate.
    let retry = store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![tool_event(session.clone(), 1)],
            },
        )
        .await
        .unwrap();
    assert_eq!(retry.duplicates, 1);
    assert_eq!(retry.rejected_conflicts, 0);

    // The stored event is still the original tool event, not the regression.
    let events = store
        .list_events(EventQuery {
            instance_id: instance_id().to_string(),
            session_id: Some(session.clone()),
            before: None,
            limit: 100,
            filter: EventFilter::default(),
        })
        .await
        .unwrap();
    assert_eq!(events.items.len(), 1);
    assert_eq!(events.items[0].tool_name.as_deref(), Some("Read"));
}

#[tokio::test]
async fn sqlite_store_search_filters_fixed_fields_without_free_text() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let first = session_id("session-a");
    let second = session_id("session-b");
    store
        .upsert_instance(&announce(vec![first.clone(), second.clone()]))
        .await
        .unwrap();
    store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![tool_event(first.clone(), 1), protocol_event(second, 1)],
            },
        )
        .await
        .unwrap();

    let hits = store
        .search(SearchQuery {
            instance: Some(instance_id().to_string()),
            session: None,
            agent: None,
            kind: None,
            inner_kind: None,
            tool: Some("Read".to_string()),
            error: None,
            q: None,
            from: None,
            to: None,
            limit: None,
            cursor: None,
        })
        .await
        .unwrap();

    assert_eq!(hits.items.len(), 1);
    assert_eq!(hits.items[0].event.session_id, first);
    assert_eq!(hits.items[0].event.msg_type, "tool_use");
}

#[tokio::test]
async fn sqlite_store_rejects_cross_instance_events_as_parse_failures() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let session = session_id("session-a");
    let mut event = protocol_event(session, 1);
    event.instance_id = Uuid::parse_str("00000000-0000-0000-0000-000000000099").unwrap();

    let stats = store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![event],
            },
        )
        .await
        .unwrap();

    assert_eq!(stats.accepted, 0);
    assert_eq!(stats.parse_failures, 1);
}

#[tokio::test]
async fn sqlite_store_retention_sweep_expires_events_and_empty_sessions_by_age() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let session = session_id("session-a");
    store
        .upsert_instance(&announce(vec![session.clone()]))
        .await
        .unwrap();
    store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![
                    protocol_event(session.clone(), 1),
                    tool_event(session.clone(), 2),
                ],
            },
        )
        .await
        .unwrap();

    let stats = store
        .run_retention_sweep(&RetentionPolicy {
            retention_days: 0,
            retention_max_bytes: i64::MAX,
        })
        .await
        .unwrap();

    assert_eq!(stats.deleted_events, 2);
    assert_eq!(stats.deleted_sessions, 1);
    assert!(
        store
            .get_event(&instance_id().to_string(), session.as_str(), 1)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_session(&instance_id().to_string(), session.as_str())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sqlite_store_retention_sweep_enforces_size_cap_by_dropping_oldest_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(tmp.path().join("events.sqlite")).unwrap();
    let first = session_id("session-a");
    let second = session_id("session-b");
    store
        .upsert_instance(&announce(vec![first.clone(), second.clone()]))
        .await
        .unwrap();
    store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![
                    protocol_event(first.clone(), 1),
                    protocol_event(second.clone(), 2),
                ],
            },
        )
        .await
        .unwrap();

    let stats = store
        .run_retention_sweep(&RetentionPolicy {
            retention_days: i64::MAX / 86_400_000,
            retention_max_bytes: 1,
        })
        .await
        .unwrap();

    assert_eq!(stats.deleted_events, 2);
    assert_eq!(stats.deleted_sessions, 2);
    assert!(
        store
            .list_sessions(&instance_id().to_string(), ListSessionsParams::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn sqlite_store_size_cap_prefers_dormant_instance_over_live_session() {
    let store = SqliteEventStore::open_in_memory().unwrap();
    let dormant_session = session_id("session-a");
    let live_session = session_id("session-b");

    // Dormant instance's session carries the *newer* event; the live instance's
    // session is the older one. Pure oldest-by-timestamp would evict the live
    // session first — the dormant preference must override that.
    store
        .upsert_instance(&announce_for(instance_id(), vec![dormant_session.clone()]))
        .await
        .unwrap();
    store
        .ingest_batch(
            &instance_id().to_string(),
            BatchFrame {
                events: vec![protocol_event_for(
                    instance_id(),
                    dormant_session.clone(),
                    5,
                )],
            },
        )
        .await
        .unwrap();
    store
        .upsert_instance(&announce_for(instance_id_b(), vec![live_session.clone()]))
        .await
        .unwrap();
    store
        .ingest_batch(
            &instance_id_b().to_string(),
            BatchFrame {
                events: vec![protocol_event_for(instance_id_b(), live_session.clone(), 1)],
            },
        )
        .await
        .unwrap();

    // Backdate instance A to the epoch so it reads as dormant; instance B keeps
    // its just-announced (live) last_seen.
    store
        .set_instance_last_seen_for_test(&instance_id().to_string(), 1_000)
        .await
        .unwrap();

    let live_cutoff_ms = Utc::now().timestamp_millis() - 600_000;
    let (dormant_pick, fallback_pick) = store
        .eviction_candidates_for_test(live_cutoff_ms)
        .await
        .unwrap();

    // Primary picker chooses the dormant instance's session, never the live one,
    // even though the live session is older.
    assert_eq!(
        dormant_pick,
        Some((
            instance_id().to_string(),
            dormant_session.as_str().to_string()
        ))
    );
    // Fallback picker (only used when no dormant session remains) is pure
    // oldest-by-timestamp, which is the live session here.
    assert_eq!(
        fallback_pick,
        Some((
            instance_id_b().to_string(),
            live_session.as_str().to_string()
        ))
    );
}
