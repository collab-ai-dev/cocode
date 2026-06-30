use super::*;
use crate::config::MemoryConfig;
use crate::service::test_support::RecordingHandle;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;
use tempfile::tempdir;

fn config() -> MemoryConfig {
    MemoryConfig::default()
}

fn turn_input(message_count: i32, has_writes: bool) -> TurnInput {
    use coco_llm_types::LlmMessage;
    use coco_types::messages::{Message, UserMessage};
    use uuid::Uuid;
    TurnInput {
        fork_messages: Box::new(|| {
            vec![std::sync::Arc::new(Message::User(UserMessage {
                message: LlmMessage::user_text("hi"),
                uuid: Uuid::new_v4(),
                timestamp: String::new(),
                is_visible_in_transcript_only: false,
                is_virtual: false,
                is_compact_summary: false,
                permission_mode: None,
                origin: None,
                parent_tool_use_id: None,
            }))]
        }),
        message_count,
        last_message_id: Some("uuid".into()),
        has_memory_writes: Box::new(move || has_writes),
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

#[tokio::test]
async fn skips_when_extraction_disabled() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        extraction_enabled: false,
        ..config()
    };
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), cfg, handle.clone());
    let outcome = svc.maybe_extract(turn_input(20, false)).await;
    assert_eq!(outcome, ExtractOutcome::Skipped(SkipReason::Disabled));
    assert!(handle.calls().is_empty());
}

#[tokio::test]
async fn skips_when_main_agent_wrote_memory() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), config(), handle.clone());
    let outcome = svc.maybe_extract(turn_input(20, true)).await;
    assert_eq!(outcome, ExtractOutcome::Skipped(SkipReason::DirectWrite));
    assert!(handle.calls().is_empty());
}

#[tokio::test]
async fn direct_write_skip_advances_cursor() {
    // When the main agent wrote memory directly we still bump the
    // cursor so the next eligible turn doesn't reconsider the same
    // range.
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), config(), handle.clone());
    assert!(svc.last_cursor().await.is_none());
    let outcome = svc.maybe_extract(turn_input(20, true)).await;
    assert_eq!(outcome, ExtractOutcome::Skipped(SkipReason::DirectWrite));
    assert_eq!(svc.last_cursor().await.as_deref(), Some("uuid"));
}

#[tokio::test]
async fn throttle_skips_until_threshold() {
    let temp = tempdir().unwrap();
    let cfg = MemoryConfig {
        extraction_throttle: 3,
        ..config()
    };
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), cfg, handle.clone());

    let r1 = svc.maybe_extract(turn_input(10, false)).await;
    let r2 = svc.maybe_extract(turn_input(10, false)).await;
    let r3 = svc.maybe_extract(turn_input(10, false)).await;
    assert_eq!(r1, ExtractOutcome::Skipped(SkipReason::Throttled));
    assert_eq!(r2, ExtractOutcome::Skipped(SkipReason::Throttled));
    assert!(matches!(r3, ExtractOutcome::Completed { .. }));
    assert_eq!(handle.calls().len(), 1);
}

#[tokio::test]
async fn fires_with_constraints_and_fork_messages() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), config(), handle.clone());
    let outcome = svc.force(turn_input(20, false)).await;
    assert!(matches!(outcome, ExtractOutcome::Completed { .. }));
    let calls = handle.calls();
    assert_eq!(calls.len(), 1);
    let constraints = calls[0].constraints.as_ref().expect("constraints");
    assert_eq!(constraints.max_turns, Some(5));
    assert_eq!(
        constraints.allowed_write_roots,
        vec![temp.path().to_path_buf()]
    );
    // Fork behaviour is driven by fork_context_messages, not isolation.
    assert_eq!(calls[0].isolation, None);
    assert_eq!(calls[0].fork_context_messages.len(), 1);
    assert_eq!(
        calls[0].active_shell_tool,
        coco_types::ActiveShellTool::Disabled
    );
}

#[tokio::test]
async fn spawned_prompt_uses_apply_patch_when_configured() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let agent: coco_tool_runtime::AgentHandleRef = handle.clone();
    let svc = ExtractService::with_shared_agent_and_notices(
        temp.path().into(),
        config(),
        std::sync::Arc::new(std::sync::RwLock::new(agent)),
        std::sync::Arc::new(crate::telemetry::NoopEmitter),
        crate::notice::NoticeInbox::default(),
        crate::service::MemoryForkToolConfig::new(
            coco_types::ActiveShellTool::Disabled,
            patch_overrides(),
        ),
        "test-session".into(),
    );

    let outcome = svc.force(turn_input(20, false)).await;
    assert!(matches!(outcome, ExtractOutcome::Completed { .. }));
    let calls = handle.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].prompt.contains("apply_patch"));
    assert!(!calls[0].prompt.contains("Write"));
    assert!(!calls[0].prompt.contains("Edit requires a prior Read"));
}

#[tokio::test]
async fn drain_returns_true_when_idle() {
    let temp = tempdir().unwrap();
    let handle = std::sync::Arc::new(RecordingHandle::default());
    let svc = ExtractService::new(temp.path().into(), config(), handle);
    assert!(svc.drain(std::time::Duration::from_millis(20)).await);
}
