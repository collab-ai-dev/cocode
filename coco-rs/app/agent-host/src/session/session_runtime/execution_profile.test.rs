use super::*;
use coco_types::HookEventType;
use pretty_assertions::assert_eq;

#[test]
fn primary_installs_every_capability() {
    let p = SessionExecutionProfile::Primary;
    assert!(p.persists_history());
    assert!(p.registers_session_manager());
    assert!(p.registers_pid());
    assert!(p.runs_goals());
    assert!(p.runs_auto_memory());
    assert!(p.runs_skill_learning());
    assert!(p.runs_prompt_suggestion());
    assert!(p.runs_scheduled_tasks());
    assert!(p.runs_auto_title());
    assert!(!p.read_only_tools());
    assert_eq!(p.hook_policy(), HookExecutionPolicy::All);
}

#[test]
fn sidechat_disables_durable_and_background_capabilities() {
    let p = SessionExecutionProfile::SideChatReadOnly;
    assert!(!p.persists_history());
    assert!(!p.registers_session_manager());
    assert!(!p.registers_pid());
    assert!(!p.runs_goals());
    assert!(!p.runs_auto_memory());
    assert!(!p.runs_skill_learning());
    assert!(!p.runs_prompt_suggestion());
    assert!(!p.runs_scheduled_tasks());
    assert!(!p.runs_auto_title());
    assert!(p.read_only_tools());
    assert_eq!(p.hook_policy(), HookExecutionPolicy::ToolLifecycleOnly);
}

#[test]
fn all_policy_permits_every_hook() {
    let policy = HookExecutionPolicy::All;
    for event in [
        HookEventType::PreToolUse,
        HookEventType::SessionStart,
        HookEventType::SessionEnd,
        HookEventType::PreCompact,
        HookEventType::Stop,
    ] {
        assert!(policy.allows(event), "All should permit {event:?}");
    }
}

#[test]
fn tool_lifecycle_only_permits_only_tool_hooks() {
    let policy = HookExecutionPolicy::ToolLifecycleOnly;
    assert!(policy.allows(HookEventType::PreToolUse));
    assert!(policy.allows(HookEventType::PostToolUse));
    assert!(policy.allows(HookEventType::PostToolUseFailure));
    // Lifecycle and every other family are suppressed, including compaction's
    // SessionStart — the exact bypass the sidechat design must prevent.
    for event in [
        HookEventType::SessionStart,
        HookEventType::SessionEnd,
        HookEventType::PreCompact,
        HookEventType::PostCompact,
        HookEventType::Stop,
        HookEventType::UserPromptSubmit,
        HookEventType::SubagentStart,
    ] {
        assert!(
            !policy.allows(event),
            "ToolLifecycleOnly must suppress {event:?}"
        );
    }
}
