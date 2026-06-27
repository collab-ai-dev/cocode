use super::*;
use crate::config::MemoryConfig;
use crate::service::test_support::RecordingHandle;
use tempfile::tempdir;

#[tokio::test]
async fn skips_when_disabled() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        dream_enabled: false,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(
        temp.path().into(),
        cfg,
        std::sync::Arc::new(RecordingHandle::default()),
    );
    let outcome = svc.maybe_consolidate(temp.path(), Vec::new, 0).await;
    assert_eq!(outcome, DreamOutcome::Skipped(SkipReason::Disabled));
}

#[tokio::test]
async fn skips_in_kairos_mode() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        kairos_mode: true,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(
        temp.path().into(),
        cfg,
        std::sync::Arc::new(RecordingHandle::default()),
    );
    let outcome = svc.maybe_consolidate(temp.path(), Vec::new, 0).await;
    assert_eq!(outcome, DreamOutcome::Skipped(SkipReason::KairosMode));
}

#[tokio::test]
async fn skips_on_session_gate() {
    let temp = tempdir().unwrap();
    let svc = DreamService::new(
        temp.path().into(),
        MemoryConfig::default(),
        std::sync::Arc::new(RecordingHandle::default()),
    );
    // No prior consolidation → time gate passes (no last mtime).
    // Closure invoked only after time gate (lazy enumerate).
    let outcome = svc
        .maybe_consolidate(temp.path(), || vec!["s1".into(), "s2".into()], 0)
        .await;
    match outcome {
        DreamOutcome::Skipped(SkipReason::SessionGate { sessions_seen }) => {
            assert_eq!(sessions_seen, 2);
        }
        other => panic!("expected SessionGate, got {other:?}"),
    }
}

#[tokio::test]
async fn fires_with_dream_constraints() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = DreamService::new(temp.path().into(), MemoryConfig::default(), handle.clone());
    let outcome = svc
        .maybe_consolidate(
            temp.path(),
            || {
                vec![
                    "s1".into(),
                    "s2".into(),
                    "s3".into(),
                    "s4".into(),
                    "s5".into(),
                ]
            },
            DreamService::now_ms(),
        )
        .await;
    assert!(matches!(outcome, DreamOutcome::Completed { .. }));
    let calls = handle.calls();
    assert_eq!(calls.len(), 1);
    let constraints = calls[0].constraints.as_ref().expect("constraints");
    // Does NOT set `maxTurns` on the fork — the consolidation agent
    // stops naturally when it has nothing left to merge. The previous
    // `Some(20)` cap silently truncated long consolidations.
    assert_eq!(constraints.max_turns, None);
    assert_eq!(
        constraints.allowed_write_roots,
        vec![temp.path().to_path_buf()]
    );
    assert_eq!(
        calls[0].active_shell_tool,
        coco_types::ActiveShellTool::Disabled
    );
}

#[tokio::test]
async fn second_call_within_time_window_skips_on_time_gate() {
    // Time gate is checked **before** scan throttle and session
    // enumeration. The first call stamps the lock mtime at "now"; the
    // second call's `hours_since` is 0, which fails the time gate before
    // scan throttle can fire. Pre-refactor this test asserted
    // ScanThrottled because gates were checked in the wrong order.
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = DreamService::new(temp.path().into(), MemoryConfig::default(), handle.clone());
    let _first = svc
        .maybe_consolidate(
            temp.path(),
            || {
                vec![
                    "s1".into(),
                    "s2".into(),
                    "s3".into(),
                    "s4".into(),
                    "s5".into(),
                ]
            },
            DreamService::now_ms(),
        )
        .await;
    let second = svc
        .maybe_consolidate(
            temp.path(),
            || {
                vec![
                    "s1".into(),
                    "s2".into(),
                    "s3".into(),
                    "s4".into(),
                    "s5".into(),
                ]
            },
            DreamService::now_ms(),
        )
        .await;
    match second {
        DreamOutcome::Skipped(SkipReason::TimeGate { hours_since }) => {
            assert!(
                hours_since < 24,
                "expected hours_since under min_hours, got {hours_since}"
            );
        }
        other => panic!("expected TimeGate after first consolidation, got {other:?}"),
    }
}

#[tokio::test]
async fn scan_throttle_blocks_when_time_gate_passes() {
    // Force-fire two consolidations under `dream_min_hours = 1` (clamp
    // floor). The first stamps the lock at "now"; for the second, we
    // pass `now + 2h` so the time gate passes. Scan throttle (10-min)
    // then short-circuits the second call. Exercises the ScanThrottled
    // branch that the time gate now masks under the standard config.
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let cfg = MemoryConfig {
        dream_min_hours: 1,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(temp.path().into(), cfg, handle.clone());
    let sessions = || {
        vec![
            "s1".into(),
            "s2".into(),
            "s3".into(),
            "s4".into(),
            "s5".into(),
        ]
    };
    let now = DreamService::now_ms();
    let _first = svc.maybe_consolidate(temp.path(), sessions, now).await;
    // 2h forward — past the 1h min — so the time gate passes; the
    // 10-min scan throttle bumped during the first call still bites.
    let second = svc
        .maybe_consolidate(temp.path(), sessions, now + 2 * 60 * 60 * 1000)
        .await;
    assert_eq!(second, DreamOutcome::Skipped(SkipReason::ScanThrottled));
}

#[tokio::test]
async fn force_bypasses_session_gate() {
    // Manual /dream parity: no sessions, fresh-start time gate, but
    // force() must still fire so the user sees consolidation.
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = DreamService::new(temp.path().into(), MemoryConfig::default(), handle.clone());
    let outcome = svc
        .force(temp.path(), Vec::new, DreamService::now_ms())
        .await;
    assert!(matches!(outcome, DreamOutcome::Completed { .. }));
    assert_eq!(handle.calls().len(), 1);
}

#[tokio::test]
async fn force_still_respects_disabled() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        dream_enabled: false,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(
        temp.path().into(),
        cfg,
        std::sync::Arc::new(RecordingHandle::default()),
    );
    let outcome = svc.force(temp.path(), Vec::new, 0).await;
    assert_eq!(outcome, DreamOutcome::Skipped(SkipReason::Disabled));
}

#[tokio::test]
async fn force_still_respects_kairos_mode() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        kairos_mode: true,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(
        temp.path().into(),
        cfg,
        std::sync::Arc::new(RecordingHandle::default()),
    );
    let outcome = svc.force(temp.path(), Vec::new, 0).await;
    assert_eq!(outcome, DreamOutcome::Skipped(SkipReason::KairosMode));
}
