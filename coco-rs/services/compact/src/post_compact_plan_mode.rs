//! Post-compact plan-mode attachment.
//!
//! When the session is in plan mode at compact time, re-emits a `plan_mode`
//! attachment with `reminderType='full'` so the model continues operating
//! in plan mode on the FIRST post-compact turn — without this, plan
//! instructions only land later via the system-reminder cadence.
//!
//! Renders the same text template the system-reminder cadence uses
//! (`coco_context::render_plan_mode_reminder`) so model-visible output stays
//! consistent across cadence and post-compact paths.

use coco_context::PlanModeAttachment;
use coco_context::ReminderType;
use coco_context::render_plan_mode_reminder;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;

/// Build a post-compact `plan_mode` attachment when the session is in plan
/// mode. Caller assembles the [`PlanModeAttachment`] from engine state
/// (workflow, phase4_variant, agent counts, plan file path, plan_exists,
/// is_sub_agent) — this crate stays free of plan-mode resolution knobs.
///
/// Always uses `reminderType='full'` so the post-compact context gets the
/// complete instructions, not the sparse cadence variant.
pub fn create_plan_mode_attachment_if_needed(
    is_plan_mode: bool,
    mut attachment: PlanModeAttachment,
) -> Option<AttachmentMessage> {
    if !is_plan_mode {
        return None;
    }
    attachment.reminder_type = ReminderType::Full;
    let text = render_plan_mode_reminder(&attachment);
    Some(AttachmentMessage::api(
        coco_types::AttachmentKind::PlanMode,
        LlmMessage::user_text(coco_messages::wrapping::wrap_in_system_reminder(&text)),
    ))
}

#[cfg(test)]
#[path = "post_compact_plan_mode.test.rs"]
mod tests;
