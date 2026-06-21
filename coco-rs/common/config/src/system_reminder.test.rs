use super::*;
use pretty_assertions::assert_eq;

#[test]
fn default_is_fully_enabled_except_feature_gated() {
    let c = SystemReminderConfig::default();
    assert!(c.enabled);
    assert_eq!(c.timeout_ms, DEFAULT_TIMEOUT_MS);
    // Cadence / core reminders — default on.
    assert!(c.attachments.plan_mode);
    assert!(c.attachments.plan_mode_exit);
    assert!(c.attachments.plan_mode_reentry);
    assert!(c.attachments.auto_mode_exit);
    assert!(c.attachments.todo_reminder);
    assert!(c.attachments.task_reminder);
    assert!(c.attachments.critical_system_reminder);
    assert!(c.attachments.auto_mode);
    assert!(c.attachments.compaction_reminder);
    assert!(c.attachments.date_change);
    assert!(c.attachments.budget_usd);
    // Phase 2 deltas — fire unconditionally in TS; on by default.
    assert!(c.attachments.deferred_tools_delta);
    assert!(c.attachments.agent_listing_delta);
    assert!(c.attachments.mcp_instructions_delta);
    // Phase 3 cross-crate reminders — on by default (generator short-circuits
    // when ctx state is empty).
    assert!(c.attachments.hook_success);
    assert!(c.attachments.hook_blocking_error);
    assert!(c.attachments.hook_additional_context);
    assert!(c.attachments.hook_stopped_continuation);
    assert!(c.attachments.async_hook_response);
    assert!(c.attachments.diagnostics);
    assert!(c.attachments.output_style);
    assert!(c.attachments.queued_command);
    assert!(c.attachments.task_status);
    assert!(c.attachments.skill_listing);
    assert!(c.attachments.invoked_skills);
    assert!(c.attachments.teammate_mailbox);
    assert!(c.attachments.team_context);
    assert!(c.attachments.agent_pending_messages);
    // Phase 4 user-input tier — on by default (UserPrompt tier gates on user_input).
    assert!(c.attachments.at_mentioned_files);
    assert!(c.attachments.mcp_resources);
    assert!(c.attachments.agent_mentions);
    assert!(c.attachments.ide_selection);
    assert!(c.attachments.ide_opened_file);
    assert!(c.attachments.nested_memory);
    assert!(c.attachments.relevant_memories);
    // Verify-plan config defaults on; the generator still requires the
    // matching tool to be visible before it emits.
    assert!(c.attachments.verify_plan_reminder);
    // TS feature-gated reminders — opt-in (default false) to match TS
    // external-build behavior.
    assert!(!c.attachments.ultrathink_effort);
    assert!(!c.attachments.token_usage);
    assert!(!c.attachments.output_token_usage);
    assert!(!c.attachments.companion_intro);
    assert_eq!(c.critical_instruction, None);
}

#[test]
fn default_timeout_matches_ts_1000ms() {
    // TS attachments.ts:767 uses a 1000ms AbortController.
    assert_eq!(DEFAULT_TIMEOUT_MS, 1000);
}

#[test]
fn serde_roundtrip_preserves_all_fields() {
    let original = SystemReminderConfig {
        enabled: false,
        timeout_ms: 2000,
        attachments: AttachmentSettings {
            plan_mode: false,
            plan_mode_exit: true,
            plan_mode_reentry: false,
            auto_mode_exit: true,
            todo_reminder: true,
            task_reminder: false,
            critical_system_reminder: true,
            auto_mode: true,
            compaction_reminder: false,
            date_change: true,
            user_context: false,
            verify_plan_reminder: true,
            ultrathink_effort: true,
            token_usage: false,
            budget_usd: true,
            output_token_usage: true,
            companion_intro: false,
            deferred_tools_delta: true,
            agent_listing_delta: false,
            mcp_instructions_delta: true,
            hook_success: true,
            hook_blocking_error: true,
            hook_additional_context: false,
            hook_stopped_continuation: true,
            async_hook_response: false,
            diagnostics: true,
            output_style: false,
            queued_command: true,
            task_status: false,
            skill_listing: true,
            invoked_skills: false,
            teammate_mailbox: true,
            team_context: false,
            agent_pending_messages: true,
            at_mentioned_files: true,
            mcp_resources: false,
            agent_mentions: true,
            ide_selection: false,
            ide_opened_file: true,
            nested_memory: true,
            relevant_memories: false,
            already_read_file: true,
            edited_image_file: false,
            skill_discovery: false,
        },
        critical_instruction: Some("be careful".to_string()),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let back: SystemReminderConfig = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back.enabled, original.enabled);
    assert_eq!(back.timeout_ms, original.timeout_ms);
    assert_eq!(back.attachments.plan_mode, original.attachments.plan_mode);
    assert_eq!(
        back.attachments.plan_mode_exit,
        original.attachments.plan_mode_exit
    );
    assert_eq!(
        back.attachments.plan_mode_reentry,
        original.attachments.plan_mode_reentry
    );
    assert_eq!(
        back.attachments.auto_mode_exit,
        original.attachments.auto_mode_exit
    );
    assert_eq!(
        back.attachments.verify_plan_reminder,
        original.attachments.verify_plan_reminder
    );
    assert_eq!(back.critical_instruction, original.critical_instruction);
}

#[test]
fn deserialize_from_empty_object_uses_defaults() {
    let c: SystemReminderConfig = serde_json::from_str("{}").expect("deserialize empty");
    let default = SystemReminderConfig::default();
    assert_eq!(c.enabled, default.enabled);
    assert_eq!(c.timeout_ms, default.timeout_ms);
    assert_eq!(c.attachments.plan_mode, default.attachments.plan_mode);
}

#[test]
fn deserialize_partial_fills_missing_with_defaults() {
    let c: SystemReminderConfig =
        serde_json::from_str(r#"{"enabled": false}"#).expect("deserialize partial");
    assert!(!c.enabled);
    assert_eq!(c.timeout_ms, DEFAULT_TIMEOUT_MS);
    // attachments uses its own default
    assert!(c.attachments.plan_mode);
}

#[test]
fn deserialize_can_disable_verify_plan_via_user_flag() {
    // User flip in settings.json: `{ system_reminder: { attachments: { verify_plan_reminder: false } } }`.
    let c: SystemReminderConfig =
        serde_json::from_str(r#"{"attachments":{"verify_plan_reminder":false}}"#)
            .expect("deserialize");
    assert!(!c.attachments.verify_plan_reminder);
    // Other fields stay default.
    assert!(c.enabled);
    assert!(c.attachments.plan_mode);
}
