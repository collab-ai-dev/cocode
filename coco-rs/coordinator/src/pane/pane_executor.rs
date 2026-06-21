//! Pane backend executor — wraps a PaneBackend with CLI command building
//! and mailbox integration to implement TeammateExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::PaneBackend;
use super::PaneId;
use super::TeammateExecutor;
use super::TeammateSpawnConfig;
use super::TeammateSpawnResult;
use crate::constants::TEAM_LEAD_NAME;
use crate::mailbox;
use crate::spawn;
use crate::types::BackendType;

/// Tracked teammate record.
struct SpawnedTeammate {
    pane_id: PaneId,
    /// Whether this teammate was spawned inside tmux (vs. external).
    /// Used when delivering commands to determine which tmux session to target.
    _inside_tmux: bool,
}

/// Pane backend executor — wraps PaneBackend with command building.
pub struct PaneBackendExecutor {
    backend: Arc<dyn PaneBackend>,
    spawned: RwLock<HashMap<String, SpawnedTeammate>>,
}

impl PaneBackendExecutor {
    pub fn new(backend: Arc<dyn PaneBackend>) -> Self {
        Self {
            backend,
            spawned: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl TeammateExecutor for PaneBackendExecutor {
    fn backend_type(&self) -> BackendType {
        self.backend.backend_type()
    }

    async fn is_available(&self) -> bool {
        self.backend.is_available().await
    }

    async fn spawn(&self, config: TeammateSpawnConfig) -> TeammateSpawnResult {
        let agent_id = format!("{}@{}", config.name, config.team_name);

        // Create pane
        let color = config
            .color
            .unwrap_or(crate::constants::AgentColorName::Cyan);
        let pane_result = match self.backend.create_teammate_pane(&config.name, color).await {
            Ok(r) => r,
            Err(e) => {
                return TeammateSpawnResult {
                    success: false,
                    agent_id,
                    error: Some(format!("Failed to create pane: {e}")),
                    task_id: None,
                    pane_id: None,
                };
            }
        };

        // Build CLI command
        let command = spawn::build_teammate_command(&config);

        // Send command to pane
        if let Err(e) = self
            .backend
            .send_command_to_pane(&pane_result.pane_id, &command)
            .await
        {
            let _ = self.backend.kill_pane(&pane_result.pane_id).await;
            return TeammateSpawnResult {
                success: false,
                agent_id,
                error: Some(format!("Failed to send command: {e}")),
                task_id: None,
                pane_id: None,
            };
        }

        // Write initial prompt to teammate's mailbox
        let prompt_message = mailbox::TeammateMessage {
            from: TEAM_LEAD_NAME.to_string(),
            text: config.prompt.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            read: false,
            color: None,
            summary: Some("initial task".to_string()),
        };
        let _ = mailbox::write_to_mailbox(&config.name, prompt_message, &config.team_name);

        // Track the spawned teammate
        let inside_tmux = self.backend.is_running_inside().await;
        self.spawned.write().await.insert(
            agent_id.clone(),
            SpawnedTeammate {
                pane_id: pane_result.pane_id.clone(),
                _inside_tmux: inside_tmux,
            },
        );

        TeammateSpawnResult {
            success: true,
            agent_id,
            error: None,
            task_id: None,
            pane_id: Some(pane_result.pane_id),
        }
    }

    async fn send_message(
        &self,
        agent_id: &str,
        message: mailbox::TeammateMessage,
    ) -> crate::Result<()> {
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
        let spawned = self.spawned.write().await.remove(agent_id);
        if let Some(teammate) = spawned {
            self.backend.kill_pane(&teammate.pane_id).await
        } else {
            Ok(false)
        }
    }

    async fn is_active(&self, agent_id: &str) -> bool {
        self.spawned.read().await.contains_key(agent_id)
    }
}

#[cfg(test)]
#[path = "pane_executor.test.rs"]
mod tests;
