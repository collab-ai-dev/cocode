use super::*;
use uuid::Uuid;

fn fresh_range() -> StagedRange {
    StagedRange {
        start_uuid: Uuid::new_v4(),
        end_uuid: Uuid::new_v4(),
        summary: "summary text".into(),
        risk: 0.5,
        staged_at: 1_700_000_000_000,
    }
}

#[test]
fn test_ledger_stage_then_commit_produces_entry() {
    let session = Uuid::new_v4();
    let mut ledger = StagedCompactLedger::new();
    ledger.stage(session, fresh_range());
    assert_eq!(ledger.snapshot.as_ref().unwrap().staged.len(), 1);
    let summary_uuid = Uuid::new_v4();
    let entry = ledger
        .commit(session, 0, summary_uuid, "<collapsed/>".into())
        .expect("commit returns entry");
    assert_eq!(entry.session_id, session);
    assert_eq!(entry.summary_uuid, summary_uuid);
    assert_eq!(ledger.commits.len(), 1);
    assert!(ledger.snapshot.as_ref().unwrap().staged.is_empty());
}

#[test]
fn test_ledger_drain_overflow_commits_all() {
    let session = Uuid::new_v4();
    let mut ledger = StagedCompactLedger::new();
    ledger.stage(session, fresh_range());
    ledger.stage(session, fresh_range());
    ledger.stage(session, fresh_range());
    let drained = ledger.drain_overflow(session, |_| Uuid::new_v4());
    assert_eq!(drained.len(), 3);
    assert_eq!(ledger.commits.len(), 3);
    assert!(ledger.snapshot.as_ref().unwrap().staged.is_empty());
}

#[test]
fn test_ledger_reset_clears_all_state() {
    let session = Uuid::new_v4();
    let mut ledger = StagedCompactLedger::new();
    ledger.stage(session, fresh_range());
    ledger.commit(session, 0, Uuid::new_v4(), "x".into());
    assert!(!ledger.is_empty());
    ledger.reset();
    assert!(ledger.is_empty());
}

#[test]
fn test_commit_entry_round_trip_ts_camelcase() {
    let entry = CommitEntry::new(
        Uuid::nil(),
        "0000000000000001".into(),
        Uuid::nil(),
        "<collapsed/>".into(),
        "x".into(),
        Uuid::nil(),
        Uuid::nil(),
    );
    let json = serde_json::to_string(&entry).unwrap();
    assert!(json.contains("\"sessionId\":"));
    assert!(json.contains("\"collapseId\":"));
    assert!(json.contains("\"firstArchivedUuid\":"));
    assert!(json.contains("\"type\":\"marble-origami-commit\""));
}

#[test]
fn test_snapshot_entry_round_trip_ts_camelcase() {
    let snap = SnapshotEntry {
        type_: SnapshotEntry::TYPE.into(),
        session_id: Uuid::nil(),
        staged: vec![fresh_range()],
        armed: true,
        last_spawn_tokens: 1234,
    };
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"lastSpawnTokens\":1234"));
    assert!(json.contains("\"armed\":true"));
    assert!(json.contains("\"startUuid\":"));
}

fn user_msg(uuid: Uuid, text: &str) -> coco_messages::Message {
    coco_messages::create_user_message_with_uuid(uuid, text)
}

#[test]
fn test_apply_collapses_no_commits_returns_input() {
    let m1 = user_msg(Uuid::new_v4(), "a");
    let m2 = user_msg(Uuid::new_v4(), "b");
    let (out, n) = apply_collapses_if_needed(&[m1, m2], &[]);
    assert_eq!(out.len(), 2);
    assert_eq!(n, 0);
}

#[test]
fn test_apply_collapses_replaces_range_with_placeholder() {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let u3 = Uuid::new_v4();
    let summary_uuid = Uuid::new_v4();
    let messages = vec![
        user_msg(u1, "first"),
        user_msg(u2, "middle"),
        user_msg(u3, "last"),
    ];
    let commits = vec![CommitEntry::new(
        Uuid::nil(),
        "1".into(),
        summary_uuid,
        "<collapsed>X</collapsed>".into(),
        "x".into(),
        u1,
        u2,
    )];
    let (out, n) = apply_collapses_if_needed(&messages, &commits);
    assert_eq!(n, 1);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].uuid().copied().unwrap(), summary_uuid);
    assert_eq!(out[1].uuid().copied().unwrap(), u3);
}

#[test]
fn test_apply_collapses_skips_missing_range() {
    let u1 = Uuid::new_v4();
    let stale = Uuid::new_v4();
    let messages = vec![user_msg(u1, "only")];
    let commits = vec![CommitEntry::new(
        Uuid::nil(),
        "1".into(),
        Uuid::new_v4(),
        "<collapsed/>".into(),
        "s".into(),
        stale,
        stale,
    )];
    let (out, n) = apply_collapses_if_needed(&messages, &commits);
    assert_eq!(n, 0);
    assert_eq!(out.len(), 1);
}

#[test]
fn test_apply_collapses_handles_multiple_commits() {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let u3 = Uuid::new_v4();
    let u4 = Uuid::new_v4();
    let messages = vec![
        user_msg(u1, "1"),
        user_msg(u2, "2"),
        user_msg(u3, "3"),
        user_msg(u4, "4"),
    ];
    let commits = vec![
        CommitEntry::new(
            Uuid::nil(),
            "1".into(),
            Uuid::new_v4(),
            "<a/>".into(),
            "a".into(),
            u1,
            u2,
        ),
        CommitEntry::new(
            Uuid::nil(),
            "2".into(),
            Uuid::new_v4(),
            "<b/>".into(),
            "b".into(),
            u3,
            u4,
        ),
    ];
    let (out, n) = apply_collapses_if_needed(&messages, &commits);
    assert_eq!(n, 2);
    assert_eq!(out.len(), 2);
}

#[test]
fn test_collapse_id_monotonic_per_session() {
    let session = Uuid::new_v4();
    let mut ledger = StagedCompactLedger::new();
    ledger.stage(session, fresh_range());
    let e1 = ledger
        .commit(session, 0, Uuid::new_v4(), "x".into())
        .unwrap();
    ledger.stage(session, fresh_range());
    let e2 = ledger
        .commit(session, 0, Uuid::new_v4(), "x".into())
        .unwrap();
    assert_ne!(e1.collapse_id, e2.collapse_id);
}
