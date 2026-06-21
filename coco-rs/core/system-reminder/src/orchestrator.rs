//! Parallel generator orchestration with per-generator timeout.
//!
//! Each generator is a candidate, filtered by tier + config + throttle, and
//! all survivors run concurrently under a 1000ms batch timeout. Generators
//! that time out or return `Err` contribute zero reminders; the turn always
//! proceeds.
//!
//! A top-level [`tokio::time::timeout`] around `join_all` aborts the whole
//! batch simultaneously when the deadline elapses. The per-generator timeout
//! is kept as a 2x safety net so a hung generator cannot wedge the batch
//! indefinitely if the join_all polling stalls.
//!
//! Gate order (each must pass):
//!
//! 1. [`SystemReminderConfig::enabled`] — master switch.
//! 2. [`AttachmentGenerator::is_enabled`] — per-generator config flag.
//! 3. [`ReminderTier`] — filter subagent-only / user-prompt-only generators.
//!
//! Per-reminder cadence (plan/auto throttle, todo/task/verify silence
//! windows, Full-vs-Sparse) is owned by each generator's `generate()`, gated
//! on history-scan counters the engine pre-computes onto `GeneratorContext`.
//! The orchestrator no longer holds any throttle state.

use std::sync::Arc;
use std::time::Duration;

use futures::future;
use tracing::debug;

use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::ContentBlock;
use crate::types::ReminderOutput;
use crate::types::ReminderTier;
use crate::types::SystemReminder;
use coco_config::SYSTEM_REMINDER_DEFAULT_TIMEOUT_MS as DEFAULT_TIMEOUT_MS;
use coco_config::SystemReminderConfig;

const REMINDER_LOG_PREVIEW_CHARS: usize = 40;

/// The orchestrator owns the generator registry for one session. It's
/// constructed once and reused across turns. Reminder cadence is no longer
/// held here — throttled generators (plan/auto/todo/task/verify) derive it
/// from history-scan counters on `GeneratorContext`.
pub struct SystemReminderOrchestrator {
    generators: Vec<Arc<dyn AttachmentGenerator>>,
    timeout: Duration,
    config: SystemReminderConfig,
}

impl std::fmt::Debug for SystemReminderOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemReminderOrchestrator")
            .field("generators", &self.generators.len())
            .field("timeout_ms", &self.timeout.as_millis())
            .field("enabled", &self.config.enabled)
            .finish()
    }
}

impl SystemReminderOrchestrator {
    /// Construct with `config` and no generators. Callers use
    /// [`add_generator`](Self::add_generator) to register each generator —
    /// Phase B+ wires the default registry at a higher level.
    pub fn new(config: SystemReminderConfig) -> Self {
        let timeout_ms = if config.timeout_ms > 0 {
            config.timeout_ms
        } else {
            DEFAULT_TIMEOUT_MS
        };
        Self {
            generators: Vec::new(),
            timeout: Duration::from_millis(timeout_ms as u64),
            config,
        }
    }

    /// Register a generator. Generators run in parallel, but `join_all`
    /// returns results in registration order, so the registry order is also
    /// the injection order for reminders that fire on the same turn.
    pub fn add_generator(&mut self, g: Arc<dyn AttachmentGenerator>) {
        self.generators.push(g);
    }

    /// Register all built-in generators in injection order:
    /// user-input batch, all-thread batch, then main-thread batch.
    ///
    /// Engine callers use this in preference to hand-wiring each generator.
    pub fn with_default_generators(mut self) -> Self {
        self.register_default_generators();
        self
    }

    /// In-place variant of [`with_default_generators`](Self::with_default_generators).
    pub fn register_default_generators(&mut self) {
        use crate::generators::{
            AgentListingDeltaGenerator, AgentMentionsGenerator, AgentPendingMessagesGenerator,
            AlreadyReadFileGenerator, AsyncHookResponseGenerator, AtMentionedFilesGenerator,
            AutoModeEnterGenerator, AutoModeExitGenerator, BudgetUsdGenerator,
            CompactionReminderGenerator, CompanionIntroGenerator, CriticalSystemReminderGenerator,
            DateChangeGenerator, DeferredToolsDeltaGenerator, DiagnosticsGenerator,
            EditedImageFileGenerator, HookAdditionalContextGenerator, HookBlockingErrorGenerator,
            HookStoppedContinuationGenerator, HookSuccessGenerator, IdeOpenedFileGenerator,
            IdeSelectionGenerator, InvokedSkillsGenerator, McpInstructionsDeltaGenerator,
            McpResourcesGenerator, NestedMemoryGenerator, OutputStyleGenerator,
            OutputTokenUsageGenerator, PlanModeEnterGenerator, PlanModeExitGenerator,
            PlanModeReentryGenerator, RelevantMemoriesGenerator, SkillDiscoveryGenerator,
            SkillListingGenerator, TaskRemindersGenerator, TaskStatusGenerator,
            TeamContextGenerator, TeammateMailboxGenerator, TodoRemindersGenerator,
            TokenUsageGenerator, UltrathinkEffortGenerator, UserContextGenerator,
            VerifyPlanReminderGenerator,
        };

        // UserInput batch.
        self.add_generator(Arc::new(AtMentionedFilesGenerator));
        self.add_generator(Arc::new(McpResourcesGenerator));
        self.add_generator(Arc::new(AgentMentionsGenerator));
        // Audit-add — UserPrompt tier.
        self.add_generator(Arc::new(SkillDiscoveryGenerator));

        // All-thread batch, plus relevant_memories which is prefetched
        // outside the main attachment loop but renders through the same path.
        self.add_generator(Arc::new(DateChangeGenerator));
        // `prependUserContext` baseline (currentDate); fires every turn.
        self.add_generator(Arc::new(UserContextGenerator));
        self.add_generator(Arc::new(UltrathinkEffortGenerator));
        self.add_generator(Arc::new(DeferredToolsDeltaGenerator));
        self.add_generator(Arc::new(AgentListingDeltaGenerator));
        self.add_generator(Arc::new(McpInstructionsDeltaGenerator));
        self.add_generator(Arc::new(CompanionIntroGenerator));
        self.add_generator(Arc::new(NestedMemoryGenerator));
        self.add_generator(Arc::new(RelevantMemoriesGenerator));
        self.add_generator(Arc::new(SkillListingGenerator));
        self.add_generator(Arc::new(PlanModeReentryGenerator));
        self.add_generator(Arc::new(PlanModeEnterGenerator));
        self.add_generator(Arc::new(PlanModeExitGenerator));
        self.add_generator(Arc::new(AutoModeEnterGenerator));
        self.add_generator(Arc::new(AutoModeExitGenerator));
        self.add_generator(Arc::new(TodoRemindersGenerator));
        self.add_generator(Arc::new(TaskRemindersGenerator));
        self.add_generator(Arc::new(TeammateMailboxGenerator));
        self.add_generator(Arc::new(TeamContextGenerator));
        self.add_generator(Arc::new(AgentPendingMessagesGenerator));
        self.add_generator(Arc::new(CriticalSystemReminderGenerator));
        self.add_generator(Arc::new(CompactionReminderGenerator));

        // Main-thread batch, plus hook attachments produced by hook executors
        // and rendered here.
        self.add_generator(Arc::new(IdeSelectionGenerator));
        self.add_generator(Arc::new(IdeOpenedFileGenerator));
        self.add_generator(Arc::new(OutputStyleGenerator));
        self.add_generator(Arc::new(DiagnosticsGenerator));
        self.add_generator(Arc::new(TaskStatusGenerator));
        self.add_generator(Arc::new(HookSuccessGenerator));
        self.add_generator(Arc::new(HookBlockingErrorGenerator));
        self.add_generator(Arc::new(HookAdditionalContextGenerator));
        self.add_generator(Arc::new(HookStoppedContinuationGenerator));
        self.add_generator(Arc::new(AsyncHookResponseGenerator));
        self.add_generator(Arc::new(TokenUsageGenerator));
        self.add_generator(Arc::new(BudgetUsdGenerator));
        self.add_generator(Arc::new(OutputTokenUsageGenerator));
        self.add_generator(Arc::new(VerifyPlanReminderGenerator));
        self.add_generator(Arc::new(InvokedSkillsGenerator));
        // Silent reminder-native attachments: display/transcript only.
        self.add_generator(Arc::new(AlreadyReadFileGenerator));
        self.add_generator(Arc::new(EditedImageFileGenerator));

        debug!(
            count = self.generators.len(),
            generators = %self.generator_names().join(","),
            "system-reminder generators registered"
        );
    }

    /// The configured per-generator timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Number of registered generators (for diagnostics / tests).
    pub fn generator_count(&self) -> usize {
        self.generators.len()
    }

    /// The [`AttachmentType`](crate::types::AttachmentType) of every
    /// registered generator, in registration order. Used by the
    /// parity test to assert every enum variant has a default
    /// generator.
    pub fn registered_attachment_types(&self) -> Vec<crate::types::AttachmentType> {
        self.generators
            .iter()
            .map(|g| g.attachment_type())
            .collect()
    }

    /// Names of every registered generator, in registration order.
    /// Cross-checked against `AttachmentKind` coverage strings by the
    /// `every_reminder_coverage_names_a_registered_generator` test.
    pub fn generator_names(&self) -> Vec<&str> {
        self.generators.iter().map(|g| g.name()).collect()
    }

    /// Config reference (for diagnostics / tests).
    pub fn config(&self) -> &SystemReminderConfig {
        &self.config
    }

    /// Run every applicable generator and collect the reminders they produce.
    ///
    /// Generators self-gate on cadence inside `generate()` using the
    /// history-scan counters the engine put on `ctx`, so the orchestrator
    /// only applies the config + tier filters here.
    pub async fn generate_all(&self, ctx: GeneratorContext<'_>) -> Vec<SystemReminder> {
        if !self.config.enabled {
            debug!("system reminders disabled globally");
            return Vec::new();
        }

        // Filter by config + tier.
        let applicable: Vec<_> = self
            .generators
            .iter()
            .filter(|g| self.should_run(g.as_ref(), &ctx))
            .cloned()
            .collect();

        if applicable.is_empty() {
            debug!(
                human_turn = ctx.turn_number,
                "no applicable generators this turn"
            );
            return Vec::new();
        }

        debug!(
            candidate_count = applicable.len(),
            registered_count = self.generators.len(),
            human_turn = ctx.turn_number,
            "system-reminder generation start"
        );

        let ctx_ref = &ctx;
        // Per-generator deadline = batch deadline. Per-generator hard cap as
        // a safety net is 2x the configured budget so an individual hung
        // generator cannot wedge the join_all even if the batch-level
        // timeout fires while polling.
        let batch_timeout = self.timeout;
        let per_generator_timeout = batch_timeout.saturating_mul(2);
        let futures = applicable
            .iter()
            .map(|g| run_one_generator(Arc::clone(g), per_generator_timeout, ctx_ref));
        let join = future::join_all(futures);

        // Top-level batch timeout. When it fires, generators still in flight
        // are dropped (cancelled) and we return whatever finished in time.
        let results: Vec<Option<(crate::types::AttachmentType, SystemReminder)>> =
            match tokio::time::timeout(batch_timeout, join).await {
                Ok(out) => out,
                Err(_) => {
                    tracing::warn!(
                        timeout_ms = batch_timeout.as_millis() as u64,
                        human_turn = ctx.turn_number,
                        "system-reminder batch timed out; dropping in-flight generators"
                    );
                    Vec::new()
                }
            };

        let mut reminders = Vec::new();
        for (_at, reminder) in results.into_iter().flatten() {
            reminders.push(reminder);
        }

        debug!(
            produced = reminders.len(),
            produced_types = %reminders
                .iter()
                .map(|r| r.attachment_type.as_str())
                .collect::<Vec<_>>()
                .join(","),
            human_turn = ctx.turn_number,
            "orchestrator.generate_all done"
        );
        reminders
    }

    /// Combined gate: runs when config and tier agree. Per-reminder cadence
    /// is the generator's own concern (history-scan inside `generate()`).
    #[allow(clippy::borrowed_box)]
    fn should_run(&self, g: &dyn AttachmentGenerator, ctx: &GeneratorContext<'_>) -> bool {
        if !g.is_enabled(&self.config) {
            return false;
        }
        match g.tier() {
            ReminderTier::Core => {}
            ReminderTier::MainAgentOnly => {
                if !ctx.is_main_agent {
                    return false;
                }
            }
            ReminderTier::UserPrompt => {
                if !ctx.has_user_input {
                    return false;
                }
            }
        }
        true
    }
}

/// Run one generator under a timeout, mapping every outcome to
/// `Option<(AttachmentType, SystemReminder)>`.
///
/// Extracted into a free function so the future's return type is a concrete
/// `Option<...>` — inline `async move` blocks inside `.map()` defeat
/// type inference in `future::join_all`.
async fn run_one_generator(
    generator: Arc<dyn AttachmentGenerator>,
    timeout_duration: std::time::Duration,
    ctx: &GeneratorContext<'_>,
) -> Option<(crate::types::AttachmentType, SystemReminder)> {
    let name = generator.name().to_string();
    let attachment_type = generator.attachment_type();
    match tokio::time::timeout(timeout_duration, generator.generate(ctx)).await {
        Ok(Ok(Some(reminder))) => {
            let (content_chars, content_preview, content_truncated) =
                reminder_log_content(&reminder);
            tracing::debug!(
                generator = %name,
                attachment_type = %attachment_type,
                content_chars,
                content_preview = %content_preview,
                content_truncated,
                silent = reminder.is_effectively_silent(),
                "reminder produced"
            );
            Some((attachment_type, reminder))
        }
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            tracing::warn!(generator = %name, error = %e, "generator failed");
            None
        }
        Err(_) => {
            tracing::warn!(
                generator = %name,
                timeout_ms = timeout_duration.as_millis() as u64,
                "generator timed out"
            );
            None
        }
    }
}

fn reminder_log_content(reminder: &SystemReminder) -> (usize, String, bool) {
    let content = reminder_log_text(reminder);
    let content_chars = content.chars().count();
    let compact = compact_whitespace(&content);
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(REMINDER_LOG_PREVIEW_CHARS).collect();
    let truncated = chars.next().is_some();
    (content_chars, preview, truncated)
}

fn reminder_log_text(reminder: &SystemReminder) -> String {
    match &reminder.output {
        ReminderOutput::Text(text) => text.clone(),
        ReminderOutput::Messages(messages) => {
            let mut out = String::new();
            for message in messages {
                for block in &message.blocks {
                    match block {
                        ContentBlock::Text { text } => append_log_part(&mut out, text),
                        ContentBlock::Image { media_type, .. } => {
                            append_log_part(&mut out, &format!("[image:{media_type}]"));
                        }
                        ContentBlock::ToolUse { name, .. } => {
                            append_log_part(&mut out, &format!("[tool_use:{name}]"));
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            append_log_part(&mut out, content);
                        }
                    }
                }
            }
            out
        }
        ReminderOutput::ModelAttachment { payload }
        | ReminderOutput::SilentAttachment { payload } => {
            serde_json::to_string(payload).unwrap_or_else(|_| "<unserializable>".to_string())
        }
        ReminderOutput::SkillDiscovery(payload) => {
            serde_json::to_string(payload).unwrap_or_else(|_| "<unserializable>".to_string())
        }
    }
}

fn append_log_part(out: &mut String, part: &str) {
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(part);
}

fn compact_whitespace(text: &str) -> String {
    let mut out = String::new();
    for part in text.split_whitespace() {
        append_log_part(&mut out, part);
    }
    out
}

#[cfg(test)]
#[path = "orchestrator.test.rs"]
mod tests;
