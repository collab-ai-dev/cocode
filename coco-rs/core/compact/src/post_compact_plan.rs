//! Post-compact plan-file re-injection.
//!
//! After compaction clears history, re-injects the current plan file's
//! contents so the model can continue working on the plan it had been
//! following before the context boundary.
//!
//! The attachment is wrapped in `<system-reminder>` as type
//! `plan_file_reference` and renders a verbatim text template.
//!
//! Engine calls this alongside [`crate::create_post_compact_file_attachments`]
//! during the full-compact flow.

use std::path::Path;
use std::path::PathBuf;

use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;

/// Build the `plan_file_reference` attachment message when the plan file
/// for this session has content. Returns `None` when the plan file is
/// absent or empty.
///
/// `plan_file_path` and `plan_content` are the session's pre-resolved
/// plan-file state — engine looks these up once via
/// `coco_context::get_plan_file_path` + `coco_context::get_plan` and
/// passes them in. That keeps this crate free of
/// `coco_context::plan_mode` resolution knobs (plans directory, agent
/// scoping, slugging) which already live in `core/context`.
pub fn create_plan_attachment_if_needed(
    plan_file_path: &Path,
    plan_content: Option<&str>,
) -> Option<AttachmentMessage> {
    let content = plan_content?.trim_end();
    if content.is_empty() {
        return None;
    }
    let text = format!(
        "A plan file exists from plan mode at: {path}\n\nPlan contents:\n\n{content}\n\nIf this plan is relevant to the current work and not already complete, continue working on it.",
        path = plan_file_path.display(),
    );
    Some(AttachmentMessage::api(
        coco_types::AttachmentKind::PlanFileReference,
        LlmMessage::user_text(coco_messages::wrapping::wrap_in_system_reminder(&text)),
    ))
}

/// Convenience variant for callers that already hold a `PathBuf`.
pub fn create_plan_attachment_from_owned(
    plan_file_path: PathBuf,
    plan_content: Option<String>,
) -> Option<AttachmentMessage> {
    create_plan_attachment_if_needed(&plan_file_path, plan_content.as_deref())
}

#[cfg(test)]
#[path = "post_compact_plan.test.rs"]
mod tests;
