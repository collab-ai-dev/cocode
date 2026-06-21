use super::*;
use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::ReminderTier;
use crate::types::SystemReminder;
use async_trait::async_trait;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

// ── Mock generators ──

#[derive(Debug)]
struct AlwaysGen {
    name: &'static str,
    at: AttachmentType,
    tier_override: Option<ReminderTier>,
    enabled: bool,
    call_count: AtomicI32,
}

impl AlwaysGen {
    fn core(name: &'static str, at: AttachmentType) -> Self {
        Self {
            name,
            at,
            tier_override: None,
            enabled: true,
            call_count: AtomicI32::new(0),
        }
    }
}

#[async_trait]
impl AttachmentGenerator for AlwaysGen {
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
    fn is_enabled(&self, _c: &SystemReminderConfig) -> bool {
        self.enabled
    }
    async fn generate(&self, _ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(Some(SystemReminder::new(self.at, self.name)))
    }
}

#[derive(Debug)]
struct SlowGen {
    name: &'static str,
    delay: Duration,
}

#[async_trait]
impl AttachmentGenerator for SlowGen {
    fn name(&self) -> &str {
        self.name
    }
    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::PlanMode
    }
    fn is_enabled(&self, _c: &SystemReminderConfig) -> bool {
        true
    }
    async fn generate(&self, _ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        tokio::time::sleep(self.delay).await;
        Ok(Some(SystemReminder::new(AttachmentType::PlanMode, "done")))
    }
}

// ── Basic empty-state behavior ──

#[tokio::test]
async fn empty_orchestrator_returns_nothing() {
    let o = SystemReminderOrchestrator::new(SystemReminderConfig::default());
    let cfg = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&cfg).build();
    assert_eq!(o.generate_all(ctx).await.len(), 0);
    assert_eq!(o.generator_count(), 0);
}

#[tokio::test]
async fn globally_disabled_produces_nothing() {
    let cfg = SystemReminderConfig {
        enabled: false,
        ..Default::default()
    };
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    o.add_generator(Arc::new(AlwaysGen::core("A", AttachmentType::PlanMode)));
    let ctx = GeneratorContext::builder(&cfg).build();
    assert_eq!(o.generate_all(ctx).await.len(), 0);
}

// ── Tier filtering ──

#[tokio::test]
async fn main_agent_only_skipped_for_subagent() {
    let cfg = SystemReminderConfig::default();
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    let g = AlwaysGen {
        name: "Main",
        at: AttachmentType::PlanMode,
        tier_override: Some(ReminderTier::MainAgentOnly),
        enabled: true,
        call_count: AtomicI32::new(0),
    };
    o.add_generator(Arc::new(g));
    let ctx = GeneratorContext::builder(&cfg).is_main_agent(false).build();
    assert_eq!(o.generate_all(ctx).await.len(), 0);
}

#[tokio::test]
async fn user_prompt_tier_skipped_without_input() {
    let cfg = SystemReminderConfig::default();
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    let g = AlwaysGen {
        name: "UP",
        at: AttachmentType::PlanMode,
        tier_override: Some(ReminderTier::UserPrompt),
        enabled: true,
        call_count: AtomicI32::new(0),
    };
    o.add_generator(Arc::new(g));
    let ctx = GeneratorContext::builder(&cfg)
        .has_user_input(false)
        .build();
    assert_eq!(o.generate_all(ctx).await.len(), 0);
}

#[tokio::test]
async fn user_prompt_tier_runs_with_input() {
    let cfg = SystemReminderConfig::default();
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    let g = AlwaysGen {
        name: "UP",
        at: AttachmentType::PlanMode,
        tier_override: Some(ReminderTier::UserPrompt),
        enabled: true,
        call_count: AtomicI32::new(0),
    };
    o.add_generator(Arc::new(g));
    let ctx = GeneratorContext::builder(&cfg).has_user_input(true).build();
    assert_eq!(o.generate_all(ctx).await.len(), 1);
}

#[tokio::test]
async fn core_tier_runs_everywhere() {
    let cfg = SystemReminderConfig::default();
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    o.add_generator(Arc::new(AlwaysGen::core("C", AttachmentType::PlanMode)));
    // Subagent + no user input — Core still runs.
    let ctx = GeneratorContext::builder(&cfg)
        .is_main_agent(false)
        .has_user_input(false)
        .build();
    assert_eq!(o.generate_all(ctx).await.len(), 1);
}

// Per-reminder cadence (throttle) now lives in each generator's
// `generate()` via history-scan — see `plan_mode.test.rs` /
// `auto_mode_enter.test.rs`. The orchestrator no longer gates on cadence.

// ── Timeout ──

#[tokio::test]
async fn slow_generator_times_out_and_turn_continues() {
    let cfg = SystemReminderConfig {
        timeout_ms: 100, // tight budget
        ..Default::default()
    };
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    o.add_generator(Arc::new(SlowGen {
        name: "Slow",
        delay: Duration::from_millis(400),
    }));
    let ctx = GeneratorContext::builder(&cfg).build();
    let started = Instant::now();
    let out = o.generate_all(ctx).await;
    let elapsed = started.elapsed();
    assert_eq!(out.len(), 0);
    assert!(
        elapsed < Duration::from_millis(300),
        "timeout should fire ~100ms, got {elapsed:?}"
    );
}

// ── Parallel execution ──

#[tokio::test]
async fn generators_run_in_parallel() {
    let cfg = SystemReminderConfig {
        timeout_ms: 500,
        ..Default::default()
    };
    let mut o = SystemReminderOrchestrator::new(cfg.clone());
    // Use two slow generators with different attachment_types so both survive
    // the dedup and both contribute.
    o.add_generator(Arc::new(SlowGen {
        name: "SA",
        delay: Duration::from_millis(150),
    }));
    o.add_generator(Arc::new(AlwaysGen::core(
        "SB",
        AttachmentType::PlanModeExit,
    )));

    let ctx = GeneratorContext::builder(&cfg).build();
    let started = Instant::now();
    let out = o.generate_all(ctx).await;
    let elapsed = started.elapsed();
    assert_eq!(out.len(), 2);
    // If they ran sequentially, elapsed would be ~150ms + fast. Parallel ≈ 150ms.
    assert!(
        elapsed < Duration::from_millis(250),
        "parallel exec expected, got {elapsed:?}"
    );
}

// ── Default registry ──

#[tokio::test]
async fn with_default_generators_registers_all_builtins() {
    let o =
        SystemReminderOrchestrator::new(SystemReminderConfig::default()).with_default_generators();
    assert_eq!(
        o.generator_count(),
        43,
        "QueuedCommand is drained by query finalize, not the default reminder registry"
    );
}

/// Completeness guard: every system-reminder-owned `AttachmentType` variant
/// must have a default generator registered, and no generator emits a
/// variant outside the catalog. Typed API-hidden attachment events live in
/// `coco_types::AttachmentKind`, not this reminder registry.
#[tokio::test]
async fn all_attachment_type_variants_have_default_generator() {
    use crate::types::AttachmentType;
    let o =
        SystemReminderOrchestrator::new(SystemReminderConfig::default()).with_default_generators();
    let registered: std::collections::HashSet<_> =
        o.registered_attachment_types().into_iter().collect();
    let catalog: std::collections::HashSet<_> = AttachmentType::all().iter().copied().collect();

    let missing: Vec<_> = catalog
        .difference(&registered)
        .copied()
        .filter(|ty| *ty != AttachmentType::QueuedCommand)
        .collect();
    assert!(
        missing.is_empty(),
        "AttachmentType variants without default generator: {missing:?}"
    );

    let extra: Vec<_> = registered.difference(&catalog).copied().collect();
    assert!(
        extra.is_empty(),
        "default generators registering types not in AttachmentType::all(): {extra:?}"
    );
}

/// Order guard: registration order is injection order because generators
/// run concurrently but are collected with `join_all`, which preserves input
/// order.
#[tokio::test]
async fn default_registry_order_matches_ts_attachment_batches() {
    use AttachmentType::*;

    let o =
        SystemReminderOrchestrator::new(SystemReminderConfig::default()).with_default_generators();

    assert_eq!(
        o.registered_attachment_types(),
        vec![
            // userInputAttachments
            AtMentionedFiles,
            McpResources,
            AgentMentions,
            // Audit-add UserPrompt tier
            SkillDiscovery,
            // allThreadAttachments (QueuedCommand is drained by query finalize)
            DateChange,
            UserContext,
            UltrathinkEffort,
            DeferredToolsDelta,
            AgentListingDelta,
            McpInstructionsDelta,
            CompanionIntro,
            NestedMemory,
            RelevantMemories,
            SkillListing,
            PlanModeReentry,
            PlanMode,
            PlanModeExit,
            AutoMode,
            AutoModeExit,
            TodoReminder,
            TaskReminder,
            TeammateMailbox,
            TeamContext,
            AgentPendingMessages,
            CriticalSystemReminder,
            CompactionReminder,
            // mainThreadAttachments plus hook/invoked-skill renderers
            IdeSelection,
            IdeOpenedFile,
            OutputStyle,
            Diagnostics,
            TaskStatus,
            HookSuccess,
            HookBlockingError,
            HookAdditionalContext,
            HookStoppedContinuation,
            AsyncHookResponse,
            TokenUsage,
            BudgetUsd,
            OutputTokenUsage,
            VerifyPlanReminder,
            InvokedSkills,
            // silent reminder-native attachments
            AlreadyReadFile,
            EditedImageFile,
        ]
    );
}

#[tokio::test]
async fn default_registry_plan_mode_enter_fires_when_in_plan() {
    use std::path::PathBuf;
    let cfg = SystemReminderConfig::default();
    let o = SystemReminderOrchestrator::new(cfg.clone()).with_default_generators();
    let ctx = GeneratorContext::builder(&cfg)
        .is_plan_mode(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .build();
    let reminders = o.generate_all(ctx).await;
    assert_eq!(reminders.len(), 1);
    assert_eq!(
        reminders[0].attachment_type,
        AttachmentType::PlanMode,
        "only PlanMode fires — exit/reentry/auto-exit gates all off"
    );
}

#[tokio::test]
async fn default_registry_orders_reentry_before_plan_mode() {
    use std::path::PathBuf;
    let cfg = SystemReminderConfig::default();
    let o = SystemReminderOrchestrator::new(cfg.clone()).with_default_generators();
    let ctx = GeneratorContext::builder(&cfg)
        .is_plan_mode(true)
        .is_plan_reentry(true)
        .plan_exists(true)
        .plan_file_path(Some(PathBuf::from("/tmp/plan.md")))
        .build();
    let reminders = o.generate_all(ctx).await;
    assert_eq!(
        reminders
            .iter()
            .map(|r| r.attachment_type)
            .collect::<Vec<_>>(),
        vec![AttachmentType::PlanModeReentry, AttachmentType::PlanMode]
    );
}

#[tokio::test]
async fn default_registry_exit_banners_fire_outside_their_modes() {
    let cfg = SystemReminderConfig::default();
    let o = SystemReminderOrchestrator::new(cfg.clone()).with_default_generators();
    let ctx = GeneratorContext::builder(&cfg)
        .needs_plan_mode_exit_attachment(true)
        .needs_auto_mode_exit_attachment(true)
        .build();
    let reminders = o.generate_all(ctx).await;
    assert_eq!(
        reminders
            .iter()
            .map(|r| r.attachment_type)
            .collect::<Vec<_>>(),
        vec![AttachmentType::PlanModeExit, AttachmentType::AutoModeExit]
    );
}

#[tokio::test]
async fn default_registry_suppresses_stale_exit_flags_inside_modes() {
    let cfg = SystemReminderConfig::default();
    let o = SystemReminderOrchestrator::new(cfg.clone()).with_default_generators();
    let ctx = GeneratorContext::builder(&cfg)
        .is_plan_mode(true)
        .is_auto_mode(true)
        .needs_plan_mode_exit_attachment(true)
        .needs_auto_mode_exit_attachment(true)
        .build();
    let reminders = o.generate_all(ctx).await;
    let types = reminders
        .iter()
        .map(|r| r.attachment_type)
        .collect::<Vec<_>>();
    assert!(
        !types.contains(&AttachmentType::PlanModeExit),
        "got {types:?}"
    );
    assert!(
        !types.contains(&AttachmentType::AutoModeExit),
        "got {types:?}"
    );
}
