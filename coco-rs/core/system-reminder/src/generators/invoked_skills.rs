//! `invoked_skills` generator.
//!
//! Renders each invoked skill as
//! `### Skill: ${name}\nPath: ${path}\n\n${content}`, joined by
//! `\n\n---\n\n`, prefixed by the guideline header.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

#[derive(Debug, Default)]
pub struct InvokedSkillsGenerator;

#[async_trait]
impl AttachmentGenerator for InvokedSkillsGenerator {
    fn name(&self) -> &str {
        "InvokedSkillsGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::InvokedSkills
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.invoked_skills
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        if ctx.invoked_skills.is_empty() {
            return Ok(None);
        }
        let blocks: Vec<String> = ctx
            .invoked_skills
            .iter()
            .map(|s| {
                format!(
                    "### Skill: {name}\nPath: {path}\n\n{content}",
                    name = s.name,
                    path = s.path,
                    content = s.content
                )
            })
            .collect();
        let body = format!(
            "The following skills were invoked in this session. Continue to follow these guidelines:\n\n{}",
            blocks.join("\n\n---\n\n")
        );
        Ok(Some(SystemReminder::new(
            AttachmentType::InvokedSkills,
            body,
        )))
    }
}

#[cfg(test)]
#[path = "invoked_skills.test.rs"]
mod tests;
