//! Session-build unit tests.
//!
//! `SessionRuntime::build` itself needs the whole world; these cover the pure
//! helpers it delegates to.

use super::restore_world_state;
use coco_session::InMemoryStore;
use coco_session::MetadataEntry;
use coco_session::TranscriptIo;
use coco_types::SessionId;
use coco_types::WorldStateSnapshot;
use pretty_assertions::assert_eq;

fn snapshot(model: &str) -> WorldStateSnapshot {
    WorldStateSnapshot {
        model: Some(model.to_string()),
        deferred_tools: ["WebFetch".to_string()].into_iter().collect(),
        ..Default::default()
    }
}

fn record(store: &InMemoryStore, session_id: &str, agent_id: Option<&str>, model: &str) {
    store
        .append_metadata(
            session_id,
            &MetadataEntry::WorldStateSnapshot {
                session_id: SessionId::try_new(session_id).expect("valid session id"),
                agent_id: agent_id.map(str::to_string),
                snapshot: snapshot(model),
            },
        )
        .expect("metadata append succeeds");
}

#[test]
fn test_restore_world_state_seeds_the_main_scope() {
    // The resume fix in one assertion: what the previous process recorded is
    // what the next one diffs against, so the restored history's announcements
    // are not repeated.
    let store = InMemoryStore::new();
    record(&store, "s1", None, "claude-opus-5");

    let state = restore_world_state(&store, "s1");

    assert_eq!(state.world_state_for_scope(None), snapshot("claude-opus-5"));
}

#[test]
fn test_restore_world_state_keeps_scopes_apart() {
    let store = InMemoryStore::new();
    record(&store, "s1", None, "main-model");
    record(&store, "s1", Some("agent-a"), "sub-model");

    let state = restore_world_state(&store, "s1");

    assert_eq!(state.world_state_for_scope(None), snapshot("main-model"));
    assert_eq!(
        state.world_state_for_scope(Some("agent-a")),
        snapshot("sub-model")
    );
    // An unknown scope stays empty rather than inheriting the main session's
    // baseline, so a newly spawned subagent announces its own world.
    assert!(state.world_state_for_scope(Some("agent-b")).is_empty());
}

#[test]
fn test_restore_world_state_takes_the_last_record_per_scope() {
    let store = InMemoryStore::new();
    record(&store, "s1", None, "first");
    record(&store, "s1", None, "second");

    let state = restore_world_state(&store, "s1");

    assert_eq!(state.world_state_for_scope(None), snapshot("second"));
}

#[test]
fn test_restore_world_state_on_a_fresh_session_is_a_no_op() {
    // A first run has no transcript. That must be silent and leave the state
    // untouched — not an error, and not a partially seeded baseline.
    let store = InMemoryStore::new();

    let state = restore_world_state(&store, "never-written");

    assert!(state.world_state_for_scope(None).is_empty());
}
