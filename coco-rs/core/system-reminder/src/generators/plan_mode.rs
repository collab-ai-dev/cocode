//! Plan-mode reminder generators.
//!
//! Three generators, one per attachment type:
//!
//! - [`PlanModeEnterGenerator`] → `plan_mode` (Full / Sparse cadence)
//! - [`PlanModeExitGenerator`] → `plan_mode_exit` (one-shot after exit)
//! - [`PlanModeReentryGenerator`] → `plan_mode_reentry` (one-shot on first
//!   plan turn after a prior exit)
//!
//! Text templates are rendered by `coco_context::render_plan_*`.
//!
//! These generators are thin adapters: they read flags from
//! [`GeneratorContext`], build a [`coco_context::PlanModeAttachment`] /
//! [`coco_context::PlanModeExitAttachment`], and call the renderer. Engine
//! wiring (Phase D) is responsible for populating the context fields from
//! `ToolAppState` (one-shot flags + cadence counters).

use async_trait::async_trait;
use coco_context::PlanModeAttachment;
use coco_context::PlanModeExitAttachment;
use coco_context::ReminderType;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Human turns between successive steady-state plan-mode reminders.
/// Mirrors TS `PLAN_MODE_ATTACHMENT_CONFIG.TURNS_BETWEEN_ATTACHMENTS`.
const PLAN_MODE_TURNS_BETWEEN: i32 = 5;

/// Full reminder on every Nth attachment since the last exit (1st, 6th, 11th…).
/// Mirrors TS `PLAN_MODE_ATTACHMENT_CONFIG.FULL_REMINDER_EVERY_N_ATTACHMENTS`.
const FULL_REMINDER_EVERY_N: i32 = 5;

// ---------------------------------------------------------------------------
// PlanModeEnterGenerator
// ---------------------------------------------------------------------------

/// Emits the steady-state plan-mode reminder while the engine is in plan mode.
///
/// Cadence is derived from history (no in-memory throttle):
/// - Always emit on the first plan-mode turn (no prior attachment in history).
/// - Otherwise one emission every [`PLAN_MODE_TURNS_BETWEEN`] human turns,
///   gated on [`GeneratorContext::plan_mode_turns_since_attachment`].
/// - Full on the 1st/6th/11th… attachment since the last exit (every
///   [`FULL_REMINDER_EVERY_N`]), derived from
///   [`GeneratorContext::plan_mode_attachments_since_exit`]; others Sparse.
#[derive(Debug, Default)]
pub struct PlanModeEnterGenerator;

#[async_trait]
impl AttachmentGenerator for PlanModeEnterGenerator {
    fn name(&self) -> &str {
        "PlanModeEnterGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::PlanMode
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.plan_mode
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.plan_mode_feature_enabled {
            return Ok(None);
        }
        if !ctx.is_plan_mode {
            return Ok(None);
        }

        // Throttle to one emission per PLAN_MODE_TURNS_BETWEEN human turns,
        // but always emit the first time (no prior attachment → `None`).
        if let Some(n) = ctx.plan_mode_turns_since_attachment
            && n < PLAN_MODE_TURNS_BETWEEN
        {
            return Ok(None);
        }

        // Full on the 1st/6th/11th… attachment since the last exit.
        let attachment_index = ctx.plan_mode_attachments_since_exit + 1;
        let reminder_type = if attachment_index % FULL_REMINDER_EVERY_N == 1 {
            ReminderType::Full
        } else {
            ReminderType::Sparse
        };

        let attachment = build_plan_mode_attachment(ctx, reminder_type);
        let text = coco_context::render_plan_mode_reminder(&attachment);
        Ok(Some(SystemReminder::new(AttachmentType::PlanMode, text)))
    }
}

// ---------------------------------------------------------------------------
// PlanModeExitGenerator
// ---------------------------------------------------------------------------

/// One-shot `## Exited Plan Mode` banner.
///
/// Fires when the engine has set
/// [`GeneratorContext::needs_plan_mode_exit_attachment`]. That flag is set by
/// `ExitPlanModeTool` on success and by the engine when it detects an
/// unannounced plan→non-plan transition. Phase D wiring clears the flag
/// after the orchestrator consumes it — this generator does not mutate ctx.
///
/// No throttle (one-shot per flag set). If the flag somehow stays set across
/// multiple turns, the generator will keep emitting — the engine is
/// responsible for clearing it. A stale flag is suppressed while the engine
/// is still in plan mode.
#[derive(Debug, Default)]
pub struct PlanModeExitGenerator;

#[async_trait]
impl AttachmentGenerator for PlanModeExitGenerator {
    fn name(&self) -> &str {
        "PlanModeExitGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::PlanModeExit
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.plan_mode_exit
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.plan_mode_feature_enabled {
            return Ok(None);
        }
        if !ctx.needs_plan_mode_exit_attachment {
            return Ok(None);
        }
        if ctx.is_plan_mode {
            return Ok(None);
        }

        let attachment = PlanModeExitAttachment {
            plan_file_path: plan_file_path_string(ctx),
            plan_exists: ctx.plan_exists,
            outcome: ctx.pending_plan_mode_exit_outcome,
        };
        let text = coco_context::render_plan_mode_exit_reminder(&attachment);
        Ok(Some(SystemReminder::new(
            AttachmentType::PlanModeExit,
            text,
        )))
    }
}

// ---------------------------------------------------------------------------
// PlanModeReentryGenerator
// ---------------------------------------------------------------------------

/// One-shot "re-entering plan mode" banner.
///
/// Fires only when all conditions hold:
/// - `is_plan_mode` (we're in plan mode this turn)
/// - `is_plan_reentry` (engine detected a prior exit)
/// - `plan_exists` (there's an existing plan file to reference)
/// - `!is_sub_agent` — sub-agents don't re-enter
///
/// Produced via the same [`coco_context::render_plan_mode_reminder`] as the
/// steady-state reminder but with [`ReminderType::Reentry`], which emits
/// "## Re-entering Plan Mode" text referencing the existing plan file.
#[derive(Debug, Default)]
pub struct PlanModeReentryGenerator;

#[async_trait]
impl AttachmentGenerator for PlanModeReentryGenerator {
    fn name(&self) -> &str {
        "PlanModeReentryGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::PlanModeReentry
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.plan_mode_reentry
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if !ctx.plan_mode_feature_enabled {
            return Ok(None);
        }
        if !(ctx.is_plan_mode && ctx.is_plan_reentry && ctx.plan_exists && !ctx.is_sub_agent) {
            return Ok(None);
        }

        let attachment = build_plan_mode_attachment(ctx, ReminderType::Reentry);
        let text = coco_context::render_plan_mode_reminder(&attachment);
        Ok(Some(SystemReminder::new(
            AttachmentType::PlanModeReentry,
            text,
        )))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_plan_mode_attachment(
    ctx: &GeneratorContext<'_>,
    reminder_type: ReminderType,
) -> PlanModeAttachment {
    PlanModeAttachment {
        reminder_type,
        workflow: ctx.plan_workflow,
        phase4_variant: ctx.phase4_variant,
        explore_agent_count: ctx.explore_agent_count,
        plan_agent_count: ctx.plan_agent_count,
        explore_plan_agents_available: ctx.explore_plan_agents_available,
        is_sub_agent: ctx.is_sub_agent,
        plan_file_path: plan_file_path_string(ctx),
        plan_exists: ctx.plan_exists,
        // Resolve the plan-file tool from the model's actual loaded tools this
        // turn, so the reminder names `apply_patch` for gpt-5 and `Write`/`Edit`
        // for Claude — never a tool the model lacks.
        write_tool: coco_types::ToolName::write_tool_for(&ctx.tools),
        edit_tool: coco_types::ToolName::edit_tool_for(&ctx.tools),
        deferred_tools: ctx.deferred_tools.clone(),
    }
}

fn plan_file_path_string(ctx: &GeneratorContext<'_>) -> String {
    ctx.plan_file_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "plan_mode.test.rs"]
mod tests;
