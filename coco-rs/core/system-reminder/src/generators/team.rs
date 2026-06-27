//! Team / swarm reminder generators (3 variants).
//!
//! - [`TeammateMailboxGenerator`] — emits the pre-formatted unread-message
//!   bundle. The engine/swarm layer pre-formats and passes the final string
//!   via `ctx.teammate_mailbox`.
//! - [`TeamContextGenerator`] — one-shot "Team Coordination" injection on
//!   the first turn for a teammate. The first-turn-only gate is enforced
//!   upstream — the engine passes `Some(snapshot)` only on the first turn.
//! - [`AgentPendingMessagesGenerator`] — emits one `<system-reminder>` per
//!   pending teammate message, each wrapped via `wrapCommandText(coordinator)`.
//!   The wire-level `AttachmentKind::QueuedCommand` mapping is preserved by
//!   `From<AttachmentType> for AttachmentKind`.
//!
//! All three are Core tier and gated on agent-swarms availability upstream;
//! the engine populates `None` / empty when swarms are disabled.

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::queue_origin::QueueOrigin;
use crate::queue_origin::wrap_command_text;
use crate::types::AttachmentType;
use crate::types::ReminderMessage;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

// ---------------------------------------------------------------------------
// TeammateMailboxGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct TeammateMailboxGenerator;

#[async_trait]
impl AttachmentGenerator for TeammateMailboxGenerator {
    fn name(&self) -> &str {
        "TeammateMailboxGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TeammateMailbox
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.teammate_mailbox
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(info) = ctx.teammate_mailbox.as_ref() else {
            return Ok(None);
        };
        if info.formatted.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::TeammateMailbox,
            info.formatted.clone(),
        )))
    }
}

// ---------------------------------------------------------------------------
// TeamContextGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct TeamContextGenerator;

#[async_trait]
impl AttachmentGenerator for TeamContextGenerator {
    fn name(&self) -> &str {
        "TeamContextGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::TeamContext
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.team_context
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(t) = ctx.team_context.as_ref() else {
            return Ok(None);
        };
        // Any missing field would produce a nonsense injection,
        // so skip if team_name or agent_id is empty.
        if t.team_name.is_empty() || t.agent_id.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::TeamContext,
            render_team_context(t),
        )))
    }
}

/// Render the team-context body with the four field substitutions inlined.
///
/// Note: only `agentName`, `teamName`, `teamConfigPath`, `taskListPath`
/// are interpolated. The `agent_id` field on
/// [`crate::TeamContextSnapshot`] is kept for future use (and to
/// gate-check), but is not surfaced in the body.
fn render_team_context(t: &crate::TeamContextSnapshot) -> String {
    format!(
        "# Team Coordination\n\n\
         You are a teammate in team \"{team}\".\n\n\
         **Your Identity:**\n\
         - Name: {name}\n\n\
         **Team Resources:**\n\
         - Team config: {cfg}\n\
         - Task list: {tasks}\n\n\
         **Team Leader:** The team lead's name is \"team-lead\". Send updates and completion notifications to them.\n\n\
         Read the team config to discover your teammates' names. Check the task list periodically. Create new tasks when work should be divided. Mark tasks resolved when complete.\n\n\
         **IMPORTANT:** Always refer to teammates by their NAME (e.g., \"team-lead\", \"analyzer\", \"researcher\"), never by UUID. When messaging, use the name directly:\n\n\
         ```json\n\
         {{\n  \"to\": \"team-lead\",\n  \"message\": \"Your message here\",\n  \"summary\": \"Brief 5-10 word preview\"\n}}\n\
         ```",
        team = t.team_name,
        name = t.agent_name,
        cfg = t.team_config_path,
        tasks = t.task_list_path,
    )
}

// ---------------------------------------------------------------------------
// AgentPendingMessagesGenerator
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct AgentPendingMessagesGenerator;

#[async_trait]
impl AttachmentGenerator for AgentPendingMessagesGenerator {
    fn name(&self) -> &str {
        "AgentPendingMessagesGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::AgentPendingMessages
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.agent_pending_messages
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        // One `queued_command` attachment is emitted per drained message,
        // each tagged with coordinator origin: N `ReminderMessage`s, each
        // becoming its own `<system-reminder>` block via the inject
        // pipeline. Coordinator framing matches `wrapCommandText`.
        //
        // The payload is just the message text. The `AgentPendingMessage.from`
        // field is intentionally not surfaced in the body — the coordinator
        // framing already signals the source.
        let messages: Vec<ReminderMessage> = ctx
            .agent_pending_messages
            .iter()
            .filter(|m| !m.text.is_empty())
            .map(|m| {
                ReminderMessage::user_text(wrap_command_text(
                    &m.text,
                    Some(&QueueOrigin::Coordinator),
                ))
            })
            .collect();
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(SystemReminder::messages(
            AttachmentType::AgentPendingMessages,
            messages,
        )))
    }
}

#[cfg(test)]
#[path = "team.test.rs"]
mod tests;
