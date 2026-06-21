use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use coco_session::TranscriptEntry;
use coco_session::TranscriptStore;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::LocalSessionJsonStore;
use crate::store::EventFilter;
use crate::store::EventQuery;
use crate::store::EventStore;
use crate::store::ListInstancesParams;
use crate::store::SearchQuery;

fn seed(memory_base: &Path, cwd: &str, session_id: &str) {
    let paths = Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        Path::new(cwd),
    ));
    let store = TranscriptStore::new(paths);
    let user = TranscriptEntry {
        entry_type: "user".into(),
        uuid: "u1".into(),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: session_id.into(),
        cwd: cwd.into(),
        timestamp: "2026-05-17T01:00:00Z".into(),
        version: Some("0.0.0".into()),
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(json!({"role":"user","content":[{"type":"text","text":"inspect logs"}]})),
        usage: None,
        model: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    let assistant = TranscriptEntry {
        entry_type: "assistant".into(),
        uuid: "a1".into(),
        parent_uuid: Some("u1".into()),
        logical_parent_uuid: None,
        session_id: session_id.into(),
        cwd: cwd.into(),
        timestamp: "2026-05-17T01:00:02Z".into(),
        version: Some("0.0.0".into()),
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(json!({
            "role":"assistant",
            "content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.txt"}}]
        })),
        usage: Some(coco_session::TranscriptUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        }),
        model: Some("test-model".into()),
        cost_usd: Some(0.01),
        extra: serde_json::Map::new(),
    };
    store.append_message(session_id, &user).unwrap();
    store.append_message(session_id, &assistant).unwrap();
}

#[tokio::test]
async fn list_instances_derives_projects_from_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), "/tmp/project-a", "session-a");
    let store = LocalSessionJsonStore::new(tmp.path().to_path_buf());

    let instances = store
        .list_instances(ListInstancesParams::default())
        .await
        .unwrap()
        .items;

    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].session_count, 1);
    assert_eq!(instances[0].kind, "local_transcripts");
}

#[tokio::test]
async fn list_events_denormalizes_tool_use_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), "/tmp/project-a", "session-a");
    let store = LocalSessionJsonStore::new(tmp.path().to_path_buf());
    let instance_id = store
        .list_instances(ListInstancesParams::default())
        .await
        .unwrap()
        .items[0]
        .instance_id
        .clone();

    let events = store
        .list_events(EventQuery {
            instance_id,
            session_id: Some("session-a".to_string()),
            before: None,
            limit: 100,
            filter: EventFilter::default(),
        })
        .await
        .unwrap();

    assert_eq!(events.items.len(), 2);
    assert_eq!(events.items[1].kind, "transcript");
    assert_eq!(events.items[1].inner_kind.as_deref(), Some("assistant"));
    assert_eq!(events.items[1].tool_name.as_deref(), Some("Read"));
    assert_eq!(events.items[1].call_id.as_deref(), Some("toolu_1"));
}

#[tokio::test]
async fn search_filters_by_tool_without_free_text_storage() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), "/tmp/project-a", "session-a");
    let store = LocalSessionJsonStore::new(tmp.path().to_path_buf());

    let rows = store
        .search(SearchQuery {
            instance: None,
            session: None,
            kind: None,
            inner_kind: None,
            tool: Some("Read".into()),
            error: None,
            q: None,
            agent: None,
            from: None,
            to: None,
            limit: None,
            cursor: None,
        })
        .await
        .unwrap();

    assert_eq!(rows.items.len(), 1);
    assert_eq!(rows.items[0].event.tool_name.as_deref(), Some("Read"));
}

#[tokio::test]
async fn list_events_returns_stable_pagination_cursors() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path(), "/tmp/project-a", "session-a");
    let store = LocalSessionJsonStore::new(tmp.path().to_path_buf());
    let instance_id = store
        .list_instances(ListInstancesParams::default())
        .await
        .unwrap()
        .items[0]
        .instance_id
        .clone();

    let first_page = store
        .list_events(EventQuery {
            instance_id: instance_id.clone(),
            session_id: Some("session-a".to_string()),
            before: None,
            limit: 1,
            filter: EventFilter::default(),
        })
        .await
        .unwrap();
    let second_page = store
        .list_events(EventQuery {
            instance_id,
            session_id: Some("session-a".to_string()),
            before: first_page.next_cursor.clone(),
            limit: 1,
            filter: EventFilter::default(),
        })
        .await
        .unwrap();

    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.next_cursor.as_deref(), Some("offset:1"));
    assert_eq!(second_page.items.len(), 1);
    assert_ne!(first_page.items[0].event_id, second_page.items[0].event_id);
}

#[test]
fn event_rows_do_not_collide_after_many_content_blocks() {
    let blocks = (0..1001)
        .map(|index| json!({"type":"text","text":format!("block {index}")}))
        .collect::<Vec<_>>();
    let line = json!({
        "type": "assistant",
        "timestamp": "2026-05-17T01:00:02Z",
        "message": {"role":"assistant","content":blocks}
    })
    .to_string();
    let next_line = json!({
        "type": "assistant",
        "timestamp": "2026-05-17T01:00:03Z",
        "message": {"role":"assistant","content":[{"type":"text","text":"next line"}]}
    })
    .to_string();

    let mut rows = super::event_rows_from_line("instance", "session", 0, &line);
    rows.extend(super::event_rows_from_line(
        "instance", "session", 1, &next_line,
    ));
    let unique = rows.iter().map(|row| row.seq).collect::<HashSet<_>>();

    assert_eq!(unique.len(), rows.len());
}

#[test]
fn event_rows_redact_payload_display_and_preview() {
    let secret = "sk-ant-12345678901234567890";
    let line = json!({
        "type": "assistant",
        "timestamp": "2026-05-17T01:00:02Z",
        "message": {
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":"toolu_secret",
                "name":"Bash",
                "input":{"command":format!("curl -H 'Authorization: Bearer {secret}' https://example.test")}
            }]
        }
    })
    .to_string();

    let rows = super::event_rows_from_line("instance", "session", 0, &line);
    let event = &rows[0];
    let payload = serde_json::to_string(&event.payload).unwrap();
    let display = event.display_text.as_deref().unwrap_or_default();
    let refs = event.file_refs.join(" ");

    assert!(!payload.contains(secret));
    assert!(!display.contains(secret));
    assert!(!refs.contains(secret));
    assert!(payload.contains("[REDACTED_SECRET]"));
    assert!(display.contains("[REDACTED_SECRET]"));
}

#[test]
fn tool_result_display_concatenates_all_text_blocks() {
    let line = json!({
        "type": "user",
        "timestamp": "2026-05-17T01:00:02Z",
        "message": {
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"toolu_1",
                "content":[
                    {"type":"text","text":"first result"},
                    {"type":"text","text":"second result"}
                ]
            }]
        }
    })
    .to_string();

    let rows = super::event_rows_from_line("instance", "session", 0, &line);
    let display = rows[0].display_text.as_deref().unwrap_or_default();

    assert!(display.contains("first result"));
    assert!(display.contains("second result"));
}

#[test]
fn nested_tool_result_rows_keep_result_lane_and_tool_name() {
    let line = json!({
        "type": "tool_result",
        "timestamp": "2026-05-17T01:00:03Z",
        "message": {
            "message": {
                "role": "tool",
                "content": [{
                    "type": "tool-result",
                    "toolCallId": "call_1",
                    "toolName": "Glob",
                    "isError": false,
                    "output": {"type": "text", "value": "README.md"}
                }]
            },
            "tool_id": "Glob",
            "tool_use_id": "call_1"
        }
    })
    .to_string();

    let rows = super::event_rows_from_line("instance", "session", 0, &line);
    let event = &rows[0];

    assert_eq!(event.msg_type, "tool_result");
    assert_eq!(event.lane, "tool-result");
    assert_eq!(event.role, "tool");
    assert_eq!(event.tool_name.as_deref(), Some("Glob"));
    assert_eq!(event.call_id.as_deref(), Some("call_1"));
    assert_eq!(event.display_text.as_deref(), Some("README.md"));
}

#[test]
fn tool_call_rows_keep_request_lane_and_tool_name() {
    let line = json!({
        "type": "assistant",
        "timestamp": "2026-05-17T01:00:02Z",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool-call",
                "toolCallId": "call_1",
                "toolName": "Glob",
                "input": {"pattern": "**/README.md"}
            }]
        }
    })
    .to_string();

    let rows = super::event_rows_from_line("instance", "session", 0, &line);
    let event = &rows[0];

    assert_eq!(event.msg_type, "tool_use");
    assert_eq!(event.lane, "search");
    assert_eq!(event.tool_name.as_deref(), Some("Glob"));
    assert_eq!(event.call_id.as_deref(), Some("call_1"));
}
