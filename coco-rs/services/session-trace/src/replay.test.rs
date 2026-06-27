use super::*;
use crate::SCHEMA_VERSION;
use crate::TraceEvent;
use crate::TraceWriter;
use pretty_assertions::assert_eq;

#[test]
fn test_write_then_replay_round_trips_and_reduces_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = TraceWriter::create(dir.path(), "sess-1", 1_700_000_000_000).unwrap();
    writer
        .record(TraceEvent::TurnStarted {
            turn_id: "t1".to_string(),
        })
        .unwrap();
    writer
        .record(TraceEvent::ToolQueued {
            call_id: "c1".to_string(),
            name: "Bash".to_string(),
        })
        .unwrap();
    writer
        .record(TraceEvent::ToolStarted {
            call_id: "c1".to_string(),
            name: "Bash".to_string(),
        })
        .unwrap();
    writer
        .record(TraceEvent::ToolCompleted {
            call_id: "c1".to_string(),
            name: "Bash".to_string(),
            is_error: false,
        })
        .unwrap();
    writer.record(TraceEvent::ContextCompacted).unwrap();
    writer
        .record(TraceEvent::TurnEnded {
            turn_id: "t1".to_string(),
        })
        .unwrap();
    assert_eq!(writer.recorded_count(), 6);

    let bundle = replay_bundle(dir.path()).unwrap();
    assert_eq!(bundle.manifest.session_id, "sess-1");
    assert_eq!(bundle.manifest.schema_version, SCHEMA_VERSION);
    assert_eq!(bundle.manifest.created_unix_ms, 1_700_000_000_000);
    assert_eq!(bundle.events.len(), 6);
    assert_eq!(bundle.compaction_count, 1);
    assert_eq!(
        bundle.tool_calls.get("c1"),
        Some(&ToolCallStatus::Completed { is_error: false })
    );
}

#[test]
fn test_record_core_drops_non_durable_events() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = TraceWriter::create(dir.path(), "s", 0).unwrap();

    let durable =
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::CompactionStarted);
    let noise = coco_types::CoreEvent::Stream(coco_types::AgentStreamEvent::TextDelta {
        turn_id: "t".to_string(),
        delta: "x".to_string(),
    });

    assert!(writer.record_core(&durable).unwrap());
    assert!(!writer.record_core(&noise).unwrap());
    assert_eq!(writer.recorded_count(), 1);

    let bundle = replay_bundle(dir.path()).unwrap();
    assert_eq!(bundle.events, vec![TraceEvent::CompactionStarted]);
}
