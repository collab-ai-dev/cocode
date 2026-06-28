//! Tests for the `QueryEngineRunner`.
//!
//! `QueryEngineRunner` now holds `Arc<SessionRuntime>` whose
//! construction needs a full `RuntimeConfig` + provider clients +
//! settings layers — building one in a unit test would essentially
//! rebuild `run_sdk_mode`. End-to-end behavior is exercised via the
//! CLI integration path; `ScriptedRunner` in `dispatcher.test.rs` is
//! the unit-level stand-in for the `TurnRunner` trait contract.
//!
//! What we keep here is the compile-time Send+Sync assertion: the
//! `SdkServerState` holds `Arc<dyn TurnRunner>` across await points,
//! so dropping that guarantee would silently break dispatch.

use super::*;

#[test]
fn runner_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<QueryEngineRunner>();
}

#[test]
fn sdk_goal_status_format_matches_noninteractive_contract() {
    let mut goal = coco_types::ActiveGoal {
        condition: "finish migration".to_string(),
        iterations: 0,
        set_at_ms: 0,
        tokens_at_start: 0,
        last_reason: None,
    };
    assert_eq!(
        crate::goal_command::format_active_goal_status(&goal),
        "Goal active: finish migration (not yet evaluated)"
    );

    goal.iterations = 2;
    goal.last_reason = Some(" tests still failing\n rerun needed ".to_string());
    assert_eq!(
        crate::goal_command::format_active_goal_status(&goal),
        "Goal active: finish migration (2 turns)\ntests still failing rerun needed"
    );
}

#[test]
fn sdk_find_last_achieved_goal_skips_clear_sentinel() {
    let clear = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "cleared".to_string(),
            sentinel: true,
            ..Default::default()
        }),
    );
    let achieved = coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(coco_types::GoalStatusPayload {
            met: true,
            condition: "done".to_string(),
            iterations: Some(3),
            sentinel: false,
            ..Default::default()
        }),
    );
    let history = vec![std::sync::Arc::new(achieved), std::sync::Arc::new(clear)];

    let found = crate::goal_command::find_last_achieved_goal(&history).expect("achieved goal");

    assert_eq!(found.condition, "done");
    assert_eq!(found.iterations, Some(3));
}

#[test]
fn sdk_goal_hook_matcher_requires_managed_session_stop_prompt() {
    let mut hook = crate::goal_command::managed_goal_hook("done".to_string());

    assert!(crate::goal_command::is_managed_goal_hook(&hook));
    hook.managed_by = None;
    assert!(!crate::goal_command::is_managed_goal_hook(&hook));
    hook.managed_by = Some(coco_hooks::ManagedHookKind::Goal);
    hook.scope = coco_types::HookScope::User;
    assert!(!crate::goal_command::is_managed_goal_hook(&hook));
}
