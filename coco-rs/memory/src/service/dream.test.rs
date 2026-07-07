use super::*;
use crate::config::MemoryConfig;
use crate::service::test_support::RecordingHandle;
use crate::telemetry::AutoDreamFailurePhase;
use crate::telemetry::AutoDreamSkipReason;
use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;
use std::sync::Mutex;
use tempfile::tempdir;

struct PathWritingHandle {
    paths: Vec<std::path::PathBuf>,
    total_tool_use_count: i64,
}

struct FailingHandle {
    error: String,
}

#[derive(Default)]
struct RecordingTelemetry {
    events: Mutex<Vec<MemoryEvent>>,
}

impl crate::telemetry::MemoryTelemetryEmitter for RecordingTelemetry {
    fn emit(&self, event: MemoryEvent) {
        self.events
            .lock()
            .expect("telemetry events lock")
            .push(event);
    }
}

impl RecordingTelemetry {
    fn events(&self) -> Vec<MemoryEvent> {
        self.events.lock().expect("telemetry events lock").clone()
    }
}

fn patch_overrides() -> std::sync::Arc<ToolOverrides> {
    std::sync::Arc::new(
        ToolOverrides::none()
            .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
            .with_excluded(ToolId::Builtin(ToolName::Write))
            .with_excluded(ToolId::Builtin(ToolName::Edit)),
    )
}

#[async_trait::async_trait]
impl AgentHandle for FailingHandle {
    async fn spawn_agent(
        &self,
        _request: coco_tool_runtime::AgentSpawnRequest,
    ) -> Result<AgentSpawnResponse, String> {
        Err(self.error.clone())
    }

    async fn send_message(
        &self,
        _to: &str,
        _content: &str,
        _summary: Option<&str>,
    ) -> Result<coco_tool_runtime::TeamMessageDispatchResult, String> {
        Err("unused".into())
    }

    async fn resume_agent(
        &self,
        _agent_id: &str,
        _prompt: &str,
        _session_id: &coco_types::SessionId,
    ) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }

    async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }

    async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

#[async_trait::async_trait]
impl AgentHandle for PathWritingHandle {
    async fn spawn_agent(
        &self,
        _request: coco_tool_runtime::AgentSpawnRequest,
    ) -> Result<AgentSpawnResponse, String> {
        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::Completed,
            agent_id: Some("dream".into()),
            result: Some("ok".into()),
            total_tool_use_count: self.total_tool_use_count,
            paths_written: self.paths.clone(),
            ..Default::default()
        })
    }

    async fn send_message(
        &self,
        _to: &str,
        _content: &str,
        _summary: Option<&str>,
    ) -> Result<coco_tool_runtime::TeamMessageDispatchResult, String> {
        Err("unused".into())
    }

    async fn resume_agent(
        &self,
        _agent_id: &str,
        _prompt: &str,
        _session_id: &coco_types::SessionId,
    ) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }

    async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }

    async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
        Err("unused".into())
    }
}

#[test]
fn error_class_from_message_uses_stable_prefix() {
    assert_eq!(
        error_class_from_message("PermissionDenied: write denied"),
        "PermissionDenied"
    );
    assert_eq!(error_class_from_message("plain failure"), "Error");
    assert_eq!(error_class_from_message(""), "Error");
}

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
async fn session_gate_emits_auto_dream_skipped_telemetry() {
    let temp = tempdir().unwrap();
    let telemetry = std::sync::Arc::new(RecordingTelemetry::default());
    let handle: crate::service::extract::AgentSlot = std::sync::Arc::new(std::sync::RwLock::new(
        std::sync::Arc::new(RecordingHandle::default()),
    ));
    let svc = DreamService::with_shared_agent(
        temp.path().into(),
        MemoryConfig::default(),
        handle,
        telemetry.clone(),
    );

    let outcome = svc
        .maybe_consolidate(temp.path(), || vec!["s1".into(), "s2".into()], 0)
        .await;

    assert_eq!(
        outcome,
        DreamOutcome::Skipped(SkipReason::SessionGate { sessions_seen: 2 })
    );
    let skipped_events = telemetry
        .events()
        .into_iter()
        .filter_map(|event| match event {
            MemoryEvent::AutoDreamSkipped {
                reason,
                session_count,
                min_required,
            } => Some((reason, session_count, min_required)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        skipped_events,
        vec![(AutoDreamSkipReason::Sessions, Some(2), Some(5))]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn held_lock_emits_auto_dream_skipped_telemetry() {
    let temp = tempdir().unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn lock holder");
    let lock_path = temp.path().join(crate::lock::LOCK_FILENAME);
    std::fs::write(&lock_path, child.id().to_string()).expect("write lock pid");

    let telemetry = std::sync::Arc::new(RecordingTelemetry::default());
    let handle: crate::service::extract::AgentSlot = std::sync::Arc::new(std::sync::RwLock::new(
        std::sync::Arc::new(RecordingHandle::default()),
    ));
    let svc = DreamService::with_shared_agent(
        temp.path().into(),
        MemoryConfig::default(),
        handle,
        telemetry.clone(),
    );

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
            DreamService::now_ms() + 25 * 60 * 60 * 1000,
        )
        .await;
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(outcome, DreamOutcome::Skipped(SkipReason::LockHeld));
    let skipped_events = telemetry
        .events()
        .into_iter()
        .filter_map(|event| match event {
            MemoryEvent::AutoDreamSkipped {
                reason,
                session_count,
                min_required,
            } => Some((reason, session_count, min_required)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        skipped_events,
        vec![(AutoDreamSkipReason::Lock, None, None)]
    );
}

#[tokio::test]
async fn session_gate_attempt_sets_scan_throttle() {
    let temp = tempdir().unwrap();
    let svc = DreamService::new(
        temp.path().into(),
        MemoryConfig::default(),
        std::sync::Arc::new(RecordingHandle::default()),
    );
    let scans = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let first_scans = scans.clone();
    let first = svc
        .maybe_consolidate(
            temp.path(),
            move || {
                first_scans.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                vec!["s1".into(), "s2".into()]
            },
            DreamService::now_ms(),
        )
        .await;
    assert!(matches!(
        first,
        DreamOutcome::Skipped(SkipReason::SessionGate { sessions_seen: 2 })
    ));

    let second_scans = scans.clone();
    let second = svc
        .maybe_consolidate(
            temp.path(),
            move || {
                second_scans.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    assert_eq!(second, DreamOutcome::Skipped(SkipReason::ScanThrottled));
    assert_eq!(scans.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn personal_only_background_dream_fires_with_dream_constraints() {
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
    assert!(calls[0].prompt.contains("Session logs"));
    assert!(
        calls[0]
            .prompt
            .contains("Reconcile memories against CLAUDE.md")
    );
    assert!(
        !calls[0]
            .prompt
            .contains("Team memory (`team/` subdirectory)")
    );
}

#[tokio::test]
async fn team_memory_guidance_is_included_only_when_team_recall_is_enabled() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let cfg = MemoryConfig {
        team_memory_enabled: true,
        ..MemoryConfig::default()
    };
    let svc = DreamService::new(temp.path().into(), cfg, handle.clone());
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
    assert!(
        calls[0]
            .prompt
            .contains("Team memory (`team/` subdirectory)")
    );
}

#[tokio::test]
async fn spawned_prompt_uses_apply_patch_when_configured() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let agent: coco_tool_runtime::AgentHandleRef = handle.clone();
    let svc = DreamService::with_shared_agent_and_notices(
        temp.path().into(),
        MemoryConfig::default(),
        std::sync::Arc::new(std::sync::RwLock::new(agent)),
        std::sync::Arc::new(crate::telemetry::NoopEmitter),
        crate::notice::NoticeInbox::default(),
        crate::service::MemoryForkToolConfig::new(
            coco_types::ActiveShellTool::Disabled,
            patch_overrides(),
        ),
        coco_types::SessionId::try_new("test-session").unwrap(),
    );

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
    assert!(calls[0].prompt.contains("apply_patch"));
    assert!(!calls[0].prompt.contains("Write"));
    assert!(!calls[0].prompt.contains("Edit"));
}

#[tokio::test]
async fn completed_dream_queues_memory_update_for_topic_paths() {
    let temp = tempdir().unwrap();
    let topic = temp.path().join("topic.md");
    let index = temp.path().join(crate::store::ENTRYPOINT_NAME);
    let updates = crate::notice::MemoryUpdateInbox::new();
    let notices = crate::notice::NoticeInbox::new();
    let handle: crate::service::extract::AgentSlot = std::sync::Arc::new(std::sync::RwLock::new(
        std::sync::Arc::new(PathWritingHandle {
            paths: vec![index, topic.clone()],
            total_tool_use_count: 2,
        }),
    ));
    let svc = DreamService::with_shared_agent_and_notice_channels(
        temp.path().into(),
        MemoryConfig::default(),
        handle,
        std::sync::Arc::new(crate::telemetry::NoopEmitter),
        DreamNoticeChannels::new(notices.clone(), updates.clone()),
        crate::service::MemoryForkToolConfig::disabled(),
        coco_types::SessionId::try_new("test-session").unwrap(),
    );

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
    let update = updates.drain().pop().expect("memory update");
    assert_eq!(update.source, crate::notice::MemoryUpdateSource::Dream);
    assert_eq!(update.summary, "consolidated 1 memory file");
    assert_eq!(update.paths, vec![topic.display().to_string()]);
    let notice = notices.drain().pop().expect("user notice");
    assert_eq!(notice.written_paths, vec![topic.display().to_string()]);
    assert_eq!(notice.verb, crate::notice::NoticeVerb::Improved);
}

#[tokio::test]
async fn auto_dream_telemetry_reports_team_flag_and_touched_file_count() {
    let temp = tempdir().unwrap();
    let topic = temp.path().join("topic.md");
    let index = temp.path().join(crate::store::ENTRYPOINT_NAME);
    let telemetry = std::sync::Arc::new(RecordingTelemetry::default());
    let handle: crate::service::extract::AgentSlot = std::sync::Arc::new(std::sync::RwLock::new(
        std::sync::Arc::new(PathWritingHandle {
            paths: vec![index, topic],
            total_tool_use_count: 9,
        }),
    ));
    let cfg = MemoryConfig {
        team_memory_enabled: true,
        ..MemoryConfig::default()
    };
    let svc = DreamService::with_shared_agent(temp.path().into(), cfg, handle, telemetry.clone());

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
    let events = telemetry.events();
    let fired = events.iter().find_map(|event| match event {
        MemoryEvent::AutoDreamFired {
            hours_since_last: _,
            sessions_since_last,
            team_memory_enabled,
        } => Some((*sessions_since_last, *team_memory_enabled)),
        _ => None,
    });
    assert_eq!(fired, Some((5, true)));
    let completed = events.iter().find_map(|event| match event {
        MemoryEvent::AutoDreamCompleted {
            sessions_reviewed,
            files_touched_count,
            team_memory_enabled,
            daily_logs_found,
            ..
        } => Some((
            *sessions_reviewed,
            *files_touched_count,
            *team_memory_enabled,
            *daily_logs_found,
        )),
        _ => None,
    });
    assert_eq!(completed, Some((5, 2, true, 0)));
}

#[tokio::test]
async fn auto_dream_telemetry_does_not_count_tool_uses_as_touched_files() {
    let temp = tempdir().unwrap();
    let telemetry = std::sync::Arc::new(RecordingTelemetry::default());
    let handle: crate::service::extract::AgentSlot = std::sync::Arc::new(std::sync::RwLock::new(
        std::sync::Arc::new(PathWritingHandle {
            paths: vec![],
            total_tool_use_count: 9,
        }),
    ));
    let svc = DreamService::with_shared_agent(
        temp.path().into(),
        MemoryConfig::default(),
        handle,
        telemetry.clone(),
    );

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
    let files_touched_count = telemetry
        .events()
        .into_iter()
        .find_map(|event| match event {
            MemoryEvent::AutoDreamCompleted {
                files_touched_count,
                ..
            } => Some(files_touched_count),
            _ => None,
        });
    assert_eq!(files_touched_count, Some(0));
}

#[tokio::test]
async fn count_daily_logs_counts_nested_markdown_files() {
    let temp = tempdir().unwrap();
    let nested = temp.path().join("logs/2026/06");
    tokio::fs::create_dir_all(&nested)
        .await
        .expect("create nested logs");
    tokio::fs::write(temp.path().join("logs/root.md"), "")
        .await
        .expect("write root log");
    tokio::fs::write(nested.join("daily.md"), "")
        .await
        .expect("write nested log");
    tokio::fs::write(nested.join("ignore.txt"), "")
        .await
        .expect("write non-markdown log");

    assert_eq!(count_daily_logs(temp.path()).await, 2);
    assert_eq!(count_daily_logs(&temp.path().join("missing")).await, 0);
}

#[tokio::test]
async fn auto_dream_failed_telemetry_reports_phase_and_error_class() {
    let temp = tempdir().unwrap();
    let telemetry = std::sync::Arc::new(RecordingTelemetry::default());
    let handle: crate::service::extract::AgentSlot =
        std::sync::Arc::new(std::sync::RwLock::new(std::sync::Arc::new(FailingHandle {
            error: "PermissionDenied: write denied".into(),
        })));
    let svc = DreamService::with_shared_agent(
        temp.path().into(),
        MemoryConfig::default(),
        handle,
        telemetry.clone(),
    );

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

    assert_eq!(
        outcome,
        DreamOutcome::Failed {
            reason: "PermissionDenied: write denied".into(),
        }
    );
    let failed = telemetry
        .events()
        .into_iter()
        .find_map(|event| match event {
            MemoryEvent::AutoDreamFailed { phase, error_class } => Some((phase, error_class)),
            _ => None,
        });
    assert_eq!(
        failed,
        Some((AutoDreamFailurePhase::Fork, "PermissionDenied".to_string()))
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
