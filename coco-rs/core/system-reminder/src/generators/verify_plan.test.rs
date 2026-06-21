use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::ReminderTier;
use coco_config::SystemReminderConfig;

fn cfg() -> SystemReminderConfig {
    let mut c = SystemReminderConfig::default();
    c.attachments.verify_plan_reminder = true;
    c
}

fn tools_with_verify_plan_execution() -> Vec<String> {
    vec![ToolName::VerifyPlanExecution.as_str().to_string()]
}

#[tokio::test]
async fn skips_when_config_disabled() {
    let mut c = SystemReminderConfig::default();
    c.attachments.verify_plan_reminder = false;
    assert!(!VerifyPlanReminderGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_no_pending_verification() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .has_pending_plan_verification(false)
        .turns_since_plan_exit(10)
        .build();
    assert!(
        VerifyPlanReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_on_turn_count_zero() {
    // Turn count of 0 is explicitly skipped.
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .has_pending_plan_verification(true)
        .turns_since_plan_exit(0)
        .tools(tools_with_verify_plan_execution())
        .build();
    assert!(
        VerifyPlanReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_not_on_10_turn_boundary() {
    let c = cfg();
    for n in [1, 5, 9, 11, 15, 19, 21] {
        let ctx = GeneratorContext::builder(&c)
            .has_pending_plan_verification(true)
            .turns_since_plan_exit(n)
            .tools(tools_with_verify_plan_execution())
            .build();
        assert!(
            VerifyPlanReminderGenerator
                .generate(&ctx)
                .await
                .unwrap()
                .is_none(),
            "turn {n} should not fire"
        );
    }
}

#[tokio::test]
async fn skips_when_verify_plan_execution_tool_is_not_visible() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .has_pending_plan_verification(true)
        .turns_since_plan_exit(10)
        .build();
    assert!(
        VerifyPlanReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn fires_on_10_turn_boundaries_when_tool_is_visible() {
    let c = cfg();
    for n in [10, 20, 30, 100] {
        let ctx = GeneratorContext::builder(&c)
            .has_pending_plan_verification(true)
            .turns_since_plan_exit(n)
            .tools(tools_with_verify_plan_execution())
            .build();
        let r = VerifyPlanReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("turn {n} should fire"));
        assert_eq!(r.attachment_type, AttachmentType::VerifyPlanReminder);
        let text = r.content().unwrap();
        assert!(text.contains("VerifyPlanExecution"));
        assert!(text.contains("NOT the Agent tool"));
    }
}

#[tokio::test]
async fn tier_is_main_agent_only() {
    assert_eq!(
        VerifyPlanReminderGenerator.tier(),
        ReminderTier::MainAgentOnly
    );
}

#[tokio::test]
async fn identity_accessors() {
    assert_eq!(
        VerifyPlanReminderGenerator.attachment_type(),
        AttachmentType::VerifyPlanReminder
    );
    assert_eq!(
        VerifyPlanReminderGenerator.name(),
        "VerifyPlanReminderGenerator"
    );
}
