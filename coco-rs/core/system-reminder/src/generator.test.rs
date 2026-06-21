use super::*;
use crate::error::Result;
use crate::types::AttachmentType;
use crate::types::ReminderTier;
use crate::types::SystemReminder;
use async_trait::async_trait;
use pretty_assertions::assert_eq;

// ── Minimal mock generator for trait-shape tests ──

#[derive(Debug)]
struct MockGen {
    name: &'static str,
    at: AttachmentType,
    tier_override: Option<ReminderTier>,
    enabled: bool,
    output: Option<String>,
}

#[async_trait]
impl AttachmentGenerator for MockGen {
    fn name(&self) -> &str {
        self.name
    }

    fn attachment_type(&self) -> AttachmentType {
        self.at
    }

    fn tier(&self) -> ReminderTier {
        self.tier_override
            .unwrap_or_else(|| self.attachment_type().tier())
    }

    fn is_enabled(&self, _config: &SystemReminderConfig) -> bool {
        self.enabled
    }

    async fn generate(&self, _ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        Ok(self
            .output
            .as_ref()
            .map(|s| SystemReminder::new(self.at, s)))
    }
}

#[test]
fn default_tier_delegates_to_attachment_type() {
    let g = MockGen {
        name: "M",
        at: AttachmentType::PlanMode,
        tier_override: None,
        enabled: true,
        output: None,
    };
    assert_eq!(g.tier(), ReminderTier::Core);
}

#[test]
fn tier_override_takes_precedence() {
    let g = MockGen {
        name: "M",
        at: AttachmentType::PlanMode,
        tier_override: Some(ReminderTier::MainAgentOnly),
        enabled: true,
        output: None,
    };
    assert_eq!(g.tier(), ReminderTier::MainAgentOnly);
}

#[tokio::test]
async fn generate_returns_none_when_no_output() {
    let cfg = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&cfg).build();
    let g = MockGen {
        name: "M",
        at: AttachmentType::PlanMode,
        tier_override: None,
        enabled: true,
        output: None,
    };
    assert!(g.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn generate_returns_reminder_with_content() {
    let cfg = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&cfg).build();
    let g = MockGen {
        name: "M",
        at: AttachmentType::PlanMode,
        tier_override: None,
        enabled: true,
        output: Some("hello".to_string()),
    };
    let r = g.generate(&ctx).await.unwrap().expect("has reminder");
    assert_eq!(r.attachment_type, AttachmentType::PlanMode);
    assert_eq!(r.content(), Some("hello"));
}

// ── Builder defaults + setters ──

#[test]
fn builder_defaults_are_sane() {
    let cfg = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&cfg).build();
    assert_eq!(ctx.turn_number, 0);
    assert!(ctx.is_main_agent);
    assert!(!ctx.has_user_input);
    assert!(!ctx.is_plan_mode);
    assert!(!ctx.is_plan_reentry);
    assert!(!ctx.is_plan_interview_phase);
    assert!(!ctx.needs_plan_mode_exit_attachment);
    assert!(!ctx.needs_auto_mode_exit_attachment);
    assert!(!ctx.plan_exists);
    assert!(!ctx.is_sub_agent);
    assert_eq!(ctx.explore_agent_count, super::DEFAULT_EXPLORE_AGENT_COUNT);
    assert_eq!(ctx.plan_agent_count, super::DEFAULT_PLAN_AGENT_COUNT);
    assert_eq!(ctx.agent_id, None);
    assert_eq!(ctx.last_human_turn_uuid, None);
    assert_eq!(ctx.plan_mode_turns_since_attachment, None);
    assert_eq!(ctx.plan_mode_attachments_since_exit, 0);
    assert_eq!(ctx.auto_mode_turns_since_attachment, None);
    assert_eq!(ctx.auto_mode_attachments_since_exit, 0);
}

#[test]
fn agent_counts_are_clamped_at_build() {
    let cfg = SystemReminderConfig::default();
    let lo = GeneratorContext::builder(&cfg).agent_counts(0, 0).build();
    assert_eq!(lo.explore_agent_count, super::MIN_AGENTS);
    assert_eq!(lo.plan_agent_count, super::MIN_AGENTS);

    let hi = GeneratorContext::builder(&cfg)
        .agent_counts(100, 100)
        .build();
    assert_eq!(hi.explore_agent_count, super::MAX_AGENTS);
    assert_eq!(hi.plan_agent_count, super::MAX_AGENTS);
}

#[test]
fn builder_chains_all_setters() {
    use std::path::PathBuf;
    use uuid::Uuid;
    let cfg = SystemReminderConfig::default();
    let uuid = Uuid::new_v4();
    let ctx = GeneratorContext::builder(&cfg)
        .turn_number(7)
        .is_main_agent(false)
        .has_user_input(true)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .is_plan_interview_phase(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .agent_id(Some("sub-1".to_string()))
        .last_human_turn_uuid(Some(uuid))
        .user_input(Some("hello".to_string()))
        .plan_mode_attachments_since_exit(3)
        .build();
    assert_eq!(ctx.turn_number, 7);
    assert!(!ctx.is_main_agent);
    assert!(ctx.has_user_input);
    assert!(ctx.is_plan_mode);
    assert!(ctx.is_plan_reentry);
    assert!(ctx.is_plan_interview_phase);
    assert_eq!(ctx.plan_file_path, Some(PathBuf::from("/tmp/plan.md")));
    assert_eq!(ctx.agent_id.as_deref(), Some("sub-1"));
    assert_eq!(ctx.last_human_turn_uuid, Some(uuid));
    assert_eq!(ctx.user_input.as_deref(), Some("hello"));
    assert_eq!(ctx.plan_mode_attachments_since_exit, 3);
}
