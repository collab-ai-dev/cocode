//! `skill_listing` generator.
//!
//! Engine pre-renders the full listing string (bundled + project + user
//! skills, budget-clamped) and passes it via `ctx.skill_listing`.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct SkillListingGenerator;

#[async_trait]
impl AttachmentGenerator for SkillListingGenerator {
    fn name(&self) -> &str {
        "SkillListingGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::SkillListing
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.skill_listing
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(content) = ctx.skill_listing.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let body =
            format!("The following skills are available for use with the Skill tool:\n\n{content}");
        Ok(Some(SystemReminder::new(
            AttachmentType::SkillListing,
            body,
        )))
    }
}

#[cfg(test)]
#[path = "skill_listing.test.rs"]
mod tests;
