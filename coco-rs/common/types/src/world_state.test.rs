use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_content_digest_of_equal_content_is_equal() {
    assert_eq!(ContentDigest::of("hello"), ContentDigest::of("hello"));
    assert_ne!(ContentDigest::of("hello"), ContentDigest::of("hello "));
}

#[test]
fn test_content_digest_is_stable_hex_sha256() {
    // Pinned: the digest is persisted, so a change to the hash function or
    // encoding would silently invalidate every stored snapshot.
    assert_eq!(
        ContentDigest::of("abc").as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn test_default_snapshot_is_empty() {
    assert!(WorldStateSnapshot::default().is_empty());
}

#[test]
fn test_snapshot_with_any_field_set_is_not_empty() {
    let snapshot = WorldStateSnapshot {
        model: Some("claude-opus-5".to_string()),
        ..Default::default()
    };
    assert!(!snapshot.is_empty());
}

#[test]
fn test_empty_snapshot_serializes_to_empty_object() {
    // Every field is `skip_serializing_if`, so an untouched scope costs one
    // `{}` on the wire rather than five empty collections.
    assert_eq!(
        serde_json::to_string(&WorldStateSnapshot::default()).expect("snapshot serializes"),
        "{}"
    );
}

#[test]
fn test_snapshot_round_trips_through_json() {
    let snapshot = WorldStateSnapshot {
        model: Some("claude-opus-5".to_string()),
        deferred_tools: ["WebFetch".to_string(), "CronList".to_string()]
            .into_iter()
            .collect(),
        agent_types: ["Explore".to_string()].into_iter().collect(),
        mcp_servers: [(
            "github".to_string(),
            McpServerAnnouncementState {
                tool_count: 14,
                description: Some("GitHub".to_string()),
            },
        )]
        .into_iter()
        .collect(),
        mcp_instruction_digests: [("github".to_string(), ContentDigest::of("use gh"))]
            .into_iter()
            .collect(),
    };

    let json = serde_json::to_string(&snapshot).expect("snapshot serializes");
    let restored: WorldStateSnapshot = serde_json::from_str(&json).expect("snapshot deserializes");

    assert_eq!(restored, snapshot);
}

#[test]
fn test_snapshot_serialization_is_byte_stable_across_insertion_order() {
    // Ordered collections are what let `PartialEq` be the change test and the
    // persisted form be diffable; a HashMap here would reorder per process.
    let forward = WorldStateSnapshot {
        deferred_tools: ["a".to_string(), "b".to_string(), "c".to_string()]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let reverse = WorldStateSnapshot {
        deferred_tools: ["c".to_string(), "b".to_string(), "a".to_string()]
            .into_iter()
            .collect(),
        ..Default::default()
    };

    assert_eq!(
        serde_json::to_string(&forward).expect("serializes"),
        serde_json::to_string(&reverse).expect("serializes")
    );
}

#[test]
fn test_unknown_fields_from_a_newer_writer_are_ignored() {
    // A snapshot written by a build that knows more sections must still load;
    // the unknown section simply reads as "never told" and re-announces once.
    let restored: WorldStateSnapshot =
        serde_json::from_str(r#"{"model":"m","future_section":{"x":1}}"#)
            .expect("snapshot tolerates unknown sections");

    assert_eq!(restored.model.as_deref(), Some("m"));
}
