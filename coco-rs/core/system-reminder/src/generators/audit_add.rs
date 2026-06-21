//! Audit-add reminder generators (May 2026).
//!
//! Only `skill_discovery` is a model-visible reminder produced here.
//! API-hidden attachments (`command_permissions`, `dynamic_skill`,
//! `structured_output`, `max_turns_reached`) are emitted as typed
//! silent `AttachmentMessage`s by their owner crates via
//! `coco_messages::AttachmentEmitter`, drained by the engine inbox at
//! turn start. `teammate_shutdown_batch` is `RuntimeBookkeeping` until
//! the TUI collapse path lands.
//!
//! ## Implementation gap: `skill_discovery` uses keyword-match, not LLM
//!
//! The original design runs a fast LLM call against the user prompt +
//! active skill catalog to suggest skills. coco-rs ships a **local
//! substring + word-prefix heuristic** in
//! `coco_skills::SkillManager::skill_discovery` because the LLM-backed
//! AKI service isn't ported yet. The payload shape is preserved
//! (`SkillDiscoveryPayload { skills, signal, source }`) but:
//!
//! - `source = Native`
//! - `signal = "local_keyword_match"`
//!
//! When/if the LLM-backed path lands, swap the producer and update the
//! `signal` value. Until then, downstream consumers that key on `signal`
//! should treat the coco-rs value as opaque.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

// ── skill_discovery ────────────────────────────────────────────────────

/// `skill_discovery` generator — UserPrompt-tier heuristic skill suggestion.
/// The source pre-renders the discovery prompt; empty candidate lists thread `None`.
#[derive(Debug, Default)]
pub struct SkillDiscoveryGenerator;

#[async_trait]
impl AttachmentGenerator for SkillDiscoveryGenerator {
    fn name(&self) -> &str {
        "SkillDiscoveryGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::SkillDiscovery
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.skill_discovery
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(payload) = ctx.skill_discovery.clone() else {
            return Ok(None);
        };
        if payload.skills.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::skill_discovery(payload)))
    }
}
