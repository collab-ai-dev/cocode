use super::*;
use pretty_assertions::assert_eq;

fn roundtrip(record: &JourneyRecord) -> JourneyRecord {
    let json = serde_json::to_string(record).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

#[test]
fn test_skill_learned_roundtrips_with_flattened_tag() {
    let record = JourneyRecord::new(
        1_700_000_000_000,
        Some("sess-1".into()),
        JourneyEvent::SkillLearned {
            name: "fix-nextest".into(),
        },
    );
    let json = serde_json::to_value(&record).expect("to_value");
    assert_eq!(json["event"], "skill_learned");
    assert_eq!(json["name"], "fix-nextest");
    assert_eq!(json["at_ms"], 1_700_000_000_000i64);
    assert_eq!(json["session_id"], "sess-1");
    assert_eq!(roundtrip(&record), record);
}

#[test]
fn test_session_id_omitted_when_none() {
    let record = JourneyRecord::new(
        42,
        None,
        JourneyEvent::SkillPromoted {
            name: "wt-rebase".into(),
        },
    );
    let json = serde_json::to_value(&record).expect("to_value");
    assert_eq!(json.get("session_id"), None);
    assert_eq!(roundtrip(&record), record);
}

#[test]
fn test_skill_retired_carries_reason() {
    let record = JourneyRecord::new(
        7,
        None,
        JourneyEvent::SkillRetired {
            name: "parse-log".into(),
            reason: SkillRetireReason::FailureRate,
        },
    );
    let json = serde_json::to_value(&record).expect("to_value");
    assert_eq!(json["event"], "skill_retired");
    assert_eq!(json["reason"], "failure_rate");
    assert_eq!(roundtrip(&record), record);
}

#[test]
fn test_all_variants_roundtrip() {
    let variants = [
        JourneyEvent::SkillLearned { name: "a".into() },
        JourneyEvent::SkillUpdated { name: "b".into() },
        JourneyEvent::SkillPromoted { name: "c".into() },
        JourneyEvent::SkillRetired {
            name: "d".into(),
            reason: SkillRetireReason::Inactivity,
        },
        JourneyEvent::SkillRestored { name: "e".into() },
        JourneyEvent::MemoryWritten {
            files: vec!["x.md".into(), "sub/y.md".into()],
        },
        JourneyEvent::MemoryConsolidated { files_touched: 3 },
        JourneyEvent::MemoryDeleted {
            file: "z.md".into(),
        },
    ];
    for event in variants {
        let record = JourneyRecord::new(1, None, event.clone());
        assert_eq!(roundtrip(&record).event, event);
    }
}

#[test]
fn test_skill_name_accessor() {
    assert_eq!(
        JourneyEvent::SkillLearned { name: "a".into() }.skill_name(),
        Some("a")
    );
    assert_eq!(
        JourneyEvent::MemoryConsolidated { files_touched: 1 }.skill_name(),
        None
    );
}

#[test]
fn test_retire_reason_as_str() {
    assert_eq!(SkillRetireReason::Manual.as_str(), "manual");
    assert_eq!(SkillRetireReason::Inactivity.as_str(), "inactivity");
    assert_eq!(SkillRetireReason::FailureRate.as_str(), "failure_rate");
}

#[test]
fn test_corrupt_line_deserialize_fails_cleanly() {
    // A schema-drift line (unknown event) is a deserialize error the reader
    // skips — never a panic.
    let result: Result<JourneyRecord, _> =
        serde_json::from_str(r#"{"at_ms":1,"event":"nonexistent_event"}"#);
    assert!(result.is_err());
}
