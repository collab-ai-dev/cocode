//! In-process teammate executor — wraps [`crate::runner::InProcessAgentRunner`].
//!
//! Implements [`crate::pane::TeammateExecutor`] so the coordinator's
//! `BackendRegistry` can hold it as a trait object.

use std::sync::Arc;

use async_trait::async_trait;

use crate::constants::TEAM_LEAD_NAME;
use crate::mailbox;
use crate::pane::TeammateExecutor;
use crate::pane::TeammateSpawnConfig;
use crate::pane::TeammateSpawnResult;
use crate::runner;
use crate::types::BackendType;

/// In-process teammate executor — wraps `InProcessAgentRunner`.
pub struct InProcessBackend {
    runner: Arc<runner::InProcessAgentRunner>,
}

impl InProcessBackend {
    pub fn new(runner: Arc<runner::InProcessAgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl TeammateExecutor for InProcessBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::InProcess
    }

    async fn is_available(&self) -> bool {
        true // Always available
    }

    async fn spawn(&self, config: TeammateSpawnConfig) -> TeammateSpawnResult {
        use coco_types::AgentIsolation;

        let spawn_config = runner::SpawnConfig {
            name: config.name.clone(),
            team_name: config.team_name.clone(),
            prompt: config.prompt,
            color: config.color.map(|c| c.as_str().to_string()),
            plan_mode_required: config.plan_mode_required,
            model: config.model,
            working_dir: Some(config.cwd),
            system_prompt: config.system_prompt,
            allowed_tools: config.permissions,
            allow_permission_prompts: config.allow_permission_prompts,
            effort: config.effort,
            use_exact_tools: config.use_exact_tools,
            isolation: AgentIsolation::None,
            memory_scope: None,
            mcp_servers: config.mcp_servers,
            disallowed_tools: config.disallowed_tools,
            max_turns: config.max_turns,
        };

        let result = self.runner.register_agent(spawn_config).await;
        let task_id = if result.success {
            Some(format!("task-{}", result.agent_id))
        } else {
            None
        };

        TeammateSpawnResult {
            success: result.success,
            agent_id: result.agent_id,
            error: result.error,
            task_id,
            pane_id: None,
        }
    }

    async fn send_message(
        &self,
        agent_id: &str,
        message: mailbox::TeammateMessage,
    ) -> crate::Result<()> {
        // Extract agent name from "name@team" format.
        let agent_name = agent_id.split('@').next().unwrap_or(agent_id);
        let team_name = agent_id.split('@').nth(1).unwrap_or("default");
        mailbox::write_to_mailbox(agent_name, message, team_name)
    }

    async fn terminate(&self, agent_id: &str, reason: Option<&str>) -> crate::Result<bool> {
        let agent_name = agent_id.split('@').next().unwrap_or(agent_id);
        let team_name = agent_id.split('@').nth(1).unwrap_or("default");
        mailbox::send_shutdown_request(agent_name, team_name, TEAM_LEAD_NAME, reason)?;
        Ok(true)
    }

    async fn kill(&self, agent_id: &str) -> crate::Result<bool> {
        Ok(self.runner.cancel_agent(agent_id).await)
    }

    async fn is_active(&self, agent_id: &str) -> bool {
        self.runner
            .get_context(agent_id)
            .await
            .is_some_and(|ctx| !ctx.is_cancelled())
    }
}

#[cfg(test)]
#[path = "inprocess_backend.test.rs"]
mod tests;
