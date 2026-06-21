//! Coordinator-owned team roster lifecycle.
//!
//! This is the single write path for team membership and active/idle
//! transitions. Lower-level helpers in `team_file` remain as raw file I/O
//! primitives for discovery/tests, but coordinator flows should mutate
//! membership through this store.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::constants::TEAM_LEAD_NAME;
use crate::team_file;
use crate::types::BackendType;
use crate::types::TeamAllowedPath;
use crate::types::TeamFile;
use crate::types::TeamManager;
use crate::types::TeamMember;

#[derive(Debug, Clone)]
pub struct CreateTeamResult {
    pub team_name: String,
    pub lead_agent_id: String,
    pub task_list_id: String,
    pub team_file: TeamFile,
}

#[derive(Debug, Clone)]
pub struct SpawnMemberRequest {
    pub desired_name: String,
    pub team_name: String,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
    pub color: Option<String>,
    pub plan_mode_required: bool,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub mode: Option<coco_types::PermissionMode>,
}

#[derive(Debug, Clone)]
pub struct SpawnMemberReservation {
    pub team_name: String,
    pub name: String,
    pub agent_id: String,
}

#[derive(Debug, Clone)]
pub struct CommitMemberRequest {
    pub team_name: String,
    pub agent_id: String,
    pub backend_type: BackendType,
    pub pane_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetMemberActiveRequest {
    pub team_name: String,
    pub member_name: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteTeamRequest;

#[derive(Debug, Clone)]
pub struct DeleteTeamResult {
    pub team_name: Option<String>,
    pub deleted: bool,
}

/// Roster owner shared by `SwarmAgentHandle` and future coordinator
/// callbacks.
#[derive(Clone)]
pub struct TeamRosterStore {
    active_team: Arc<RwLock<Option<TeamManager>>>,
}

impl TeamRosterStore {
    pub fn new(active_team: Arc<RwLock<Option<TeamManager>>>) -> Self {
        Self { active_team }
    }

    pub async fn active_team_name(&self) -> Option<String> {
        self.active_team
            .read()
            .await
            .as_ref()
            .map(|m| m.team_name().to_string())
    }

    pub async fn create_team(
        &self,
        request: coco_tool_runtime::CreateTeamRequest,
    ) -> Result<CreateTeamResult, String> {
        if let Some(manager) = self.active_team.read().await.as_ref() {
            return Err(format!(
                "Cannot create team '{}': leader already has active team '{}'",
                request.requested_name,
                manager.team_name()
            ));
        }
        if request.leader_session_id.trim().is_empty() {
            return Err("TeamCreate requires a non-empty leader session id".to_string());
        }
        // Leader-session dedup is the in-memory `active_team` check above
        // (one team per leader, mirroring TS `appState.teamContext`). We do NOT
        // scan `~/.coco/teams/` by `lead_session_id`: a disk scan reads every
        // *other* live `coco` process's team files (and races their create /
        // delete), coupling independent sessions for no benefit — a session
        // owns exactly one process, whose `active_team` is the authority.

        let team_name = unique_team_name(&request.requested_name);
        let lead_agent_id = request
            .leader_agent_id
            .unwrap_or_else(|| format!("{TEAM_LEAD_NAME}@{team_name}"));
        let task_list_id = crate::types::sanitize_name(&team_name);
        let now = chrono::Utc::now().timestamp();
        let team_file = TeamFile {
            name: team_name.clone(),
            description: None,
            created_at: now,
            lead_agent_id: lead_agent_id.clone(),
            lead_session_id: Some(request.leader_session_id),
            hidden_pane_ids: Vec::new(),
            team_allowed_paths: request
                .allowed_paths
                .into_iter()
                .map(|p| TeamAllowedPath {
                    path: p.path,
                    tool_name: p.tool_name,
                    added_by: p.added_by,
                    added_at: p.added_at,
                })
                .collect(),
            members: vec![TeamMember {
                agent_id: lead_agent_id.clone(),
                name: TEAM_LEAD_NAME.to_string(),
                agent_type: Some("team-lead".to_string()),
                model: request.leader_model,
                prompt: None,
                color: None,
                plan_mode_required: false,
                joined_at: now,
                tmux_pane_id: String::new(),
                cwd: request.cwd.display().to_string(),
                worktree_path: None,
                session_id: None,
                subscriptions: Vec::new(),
                backend_type: Some(BackendType::InProcess),
                is_active: true,
                mode: None,
            }],
        };

        team_file::write_team_file(&team_name, &team_file)
            .map_err(|e| format!("Failed to create team '{team_name}': {e}"))?;
        if let Some(router) = request.task_list_router {
            if let Err(e) = router.route_team_task_list(&task_list_id).await {
                let _ = team_file::cleanup_team_directories(&team_name);
                return Err(format!(
                    "Failed to route task tools to team task list '{task_list_id}': {e}"
                ));
            }
        } else {
            let _ = team_file::cleanup_team_directories(&team_name);
            return Err("TeamCreate requires a team task-list router".to_string());
        }

        *self.active_team.write().await =
            Some(TeamManager::new(team_name.clone(), team_file.clone()));

        Ok(CreateTeamResult {
            team_name,
            lead_agent_id,
            task_list_id,
            team_file,
        })
    }

    pub async fn reserve_member(
        &self,
        request: SpawnMemberRequest,
    ) -> Result<SpawnMemberReservation, String> {
        let mut team_file = team_file::read_team_file(&request.team_name)
            .map_err(|e| format!("Failed to read team '{}': {e}", request.team_name))?
            .ok_or_else(|| format!("Team '{}' does not exist", request.team_name))?;
        let existing_names = team_file
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>();
        let name = crate::teammate::generate_unique_teammate_name(
            &crate::types::sanitize_name(&request.desired_name),
            &existing_names,
        );
        let agent_id = format!("{name}@{}", request.team_name);
        let member = TeamMember {
            agent_id: agent_id.clone(),
            name: name.clone(),
            agent_type: request.agent_type,
            model: request.model,
            prompt: Some(request.prompt),
            color: request.color,
            plan_mode_required: request.plan_mode_required,
            joined_at: chrono::Utc::now().timestamp(),
            tmux_pane_id: String::new(),
            cwd: request.cwd,
            worktree_path: request.worktree_path,
            session_id: None,
            subscriptions: Vec::new(),
            backend_type: None,
            is_active: false,
            mode: request.mode,
        };
        team_file.members.push(member.clone());
        team_file::write_team_file(&request.team_name, &team_file)
            .map_err(|e| format!("Failed to reserve teammate '{name}': {e}"))?;
        if let Some(manager) = self.active_team.read().await.as_ref() {
            manager.upsert_member(member).await;
        }
        Ok(SpawnMemberReservation {
            team_name: request.team_name,
            name,
            agent_id,
        })
    }

    pub async fn commit_member(&self, request: CommitMemberRequest) -> Result<TeamMember, String> {
        let mut team_file = team_file::read_team_file(&request.team_name)
            .map_err(|e| format!("Failed to read team '{}': {e}", request.team_name))?
            .ok_or_else(|| format!("Team '{}' does not exist", request.team_name))?;
        let member = team_file
            .members
            .iter_mut()
            .find(|m| m.agent_id == request.agent_id)
            .ok_or_else(|| format!("Reserved teammate '{}' not found", request.agent_id))?;
        member.is_active = true;
        member.backend_type = Some(request.backend_type);
        member.tmux_pane_id = request.pane_id.unwrap_or_default();
        member.session_id = request.session_id;
        let committed = member.clone();
        team_file::write_team_file(&request.team_name, &team_file)
            .map_err(|e| format!("Failed to commit teammate '{}': {e}", request.agent_id))?;
        if let Some(manager) = self.active_team.read().await.as_ref() {
            manager.upsert_member(committed.clone()).await;
        }
        Ok(committed)
    }

    pub async fn set_member_color(
        &self,
        team_name: &str,
        agent_id: &str,
        color: String,
    ) -> Result<(), String> {
        let mut team_file = team_file::read_team_file(team_name)
            .map_err(|e| format!("Failed to read team '{team_name}': {e}"))?
            .ok_or_else(|| format!("Team '{team_name}' does not exist"))?;
        let Some(member) = team_file
            .members
            .iter_mut()
            .find(|m| m.agent_id == agent_id)
        else {
            return Ok(());
        };
        member.color = Some(color);
        let updated = member.clone();
        team_file::write_team_file(team_name, &team_file)
            .map_err(|e| format!("Failed to update teammate color '{agent_id}': {e}"))?;
        if let Some(manager) = self.active_team.read().await.as_ref() {
            manager.upsert_member(updated).await;
        }
        Ok(())
    }

    pub async fn rollback_member(&self, team_name: &str, agent_id: &str) -> Result<bool, String> {
        let removed = team_file::remove_member_by_agent_id(team_name, agent_id)
            .map_err(|e| format!("Failed to rollback teammate '{agent_id}': {e}"))?;
        if let Some(manager) = self.active_team.read().await.as_ref() {
            manager.remove_member(agent_id).await;
        }
        Ok(removed)
    }

    pub async fn set_member_active(&self, request: SetMemberActiveRequest) -> Result<(), String> {
        let mut team_file = match team_file::read_team_file(&request.team_name)
            .map_err(|e| format!("Failed to read team '{}': {e}", request.team_name))?
        {
            Some(tf) => tf,
            None => return Ok(()),
        };
        if let Some(member) = team_file
            .members
            .iter_mut()
            .find(|m| m.name == request.member_name)
        {
            member.is_active = request.is_active;
            let updated = member.clone();
            team_file::write_team_file(&request.team_name, &team_file).map_err(|e| {
                format!(
                    "Failed to set teammate '{}' active={}: {e}",
                    request.member_name, request.is_active
                )
            })?;
            if let Some(manager) = self.active_team.read().await.as_ref() {
                manager.upsert_member(updated).await;
            }
        }
        Ok(())
    }

    /// Persist a teammate's permission mode to `team.json` and the live
    /// roster. Leader-side write-back paired with a `ModeSetRequest` to the
    /// teammate's mailbox.
    pub async fn set_member_mode(
        &self,
        team_name: &str,
        member_name: &str,
        mode: coco_types::PermissionMode,
    ) -> Result<(), String> {
        let mut team_file = match team_file::read_team_file(team_name)
            .map_err(|e| format!("Failed to read team '{team_name}': {e}"))?
        {
            Some(tf) => tf,
            None => return Ok(()),
        };
        if let Some(member) = team_file.members.iter_mut().find(|m| m.name == member_name) {
            member.mode = Some(mode);
            let updated = member.clone();
            team_file::write_team_file(team_name, &team_file).map_err(|e| {
                format!("Failed to set teammate '{member_name}' mode={mode:?}: {e}")
            })?;
            if let Some(manager) = self.active_team.read().await.as_ref() {
                manager.upsert_member(updated).await;
            }
        }
        Ok(())
    }

    /// Persist MULTIPLE teammates' permission modes to `team.json` in ONE
    /// atomic write, then upsert each changed member into the live roster.
    /// Batching avoids the read-modify-write race of looping
    /// [`Self::set_member_mode`] (N reads + N writes of the same file).
    /// Members not present in the team file, or already at the requested
    /// mode, are skipped; the file is rewritten only when at least one
    /// member actually changes.
    pub async fn set_member_modes(
        &self,
        team_name: &str,
        updates: &[(String, coco_types::PermissionMode)],
    ) -> Result<(), String> {
        let mut team_file = match team_file::read_team_file(team_name)
            .map_err(|e| format!("Failed to read team '{team_name}': {e}"))?
        {
            Some(tf) => tf,
            None => return Ok(()),
        };
        let update_map: std::collections::HashMap<&str, coco_types::PermissionMode> = updates
            .iter()
            .map(|(name, mode)| (name.as_str(), *mode))
            .collect();
        let mut changed: Vec<crate::types::TeamMember> = Vec::new();
        for member in &mut team_file.members {
            if let Some(&new_mode) = update_map.get(member.name.as_str())
                && member.mode != Some(new_mode)
            {
                member.mode = Some(new_mode);
                changed.push(member.clone());
            }
        }
        if !changed.is_empty() {
            team_file::write_team_file(team_name, &team_file)
                .map_err(|e| format!("Failed to set member modes for team '{team_name}': {e}"))?;
            if let Some(manager) = self.active_team.read().await.as_ref() {
                for member in changed {
                    manager.upsert_member(member).await;
                }
            }
        }
        Ok(())
    }

    pub async fn running_non_lead_members(&self) -> Vec<TeamMember> {
        let Some(team_name) = self.active_team_name().await else {
            return Vec::new();
        };
        let Ok(Some(team_file)) = team_file::read_team_file(&team_name) else {
            return Vec::new();
        };
        team_file
            .members
            .into_iter()
            .filter(|m| m.name != TEAM_LEAD_NAME && m.is_active)
            .collect()
    }

    pub async fn broadcast_recipients(&self, from: &str) -> Vec<String> {
        let Some(team_name) = self.active_team_name().await else {
            return Vec::new();
        };
        let Ok(Some(team_file)) = team_file::read_team_file(&team_name) else {
            return Vec::new();
        };
        team_file
            .members
            .into_iter()
            .filter(|m| m.name != from && m.name != TEAM_LEAD_NAME && m.is_active)
            .map(|m| m.name)
            .collect()
    }

    /// Delete the active team.
    ///
    /// `notifier` is the session's task-list handle (when available). On
    /// the success path — and only when the team's task-list directory was
    /// actually removed — it fires a "tasks changed" notification so any
    /// in-process subscriber refreshes its view. The notification fires
    /// only on successful directory removal (never on failure). A `None`
    /// notifier (or a
    /// failed tasks-dir removal) skips the notification.
    pub async fn delete_team(
        &self,
        _request: DeleteTeamRequest,
        notifier: Option<&dyn coco_tool_runtime::TaskListHandle>,
    ) -> Result<DeleteTeamResult, String> {
        let Some(team_name) = self.active_team_name().await else {
            return Ok(DeleteTeamResult {
                team_name: None,
                deleted: false,
            });
        };
        let non_lead = self.running_non_lead_members().await;
        if !non_lead.is_empty() {
            let names = non_lead
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!("Cannot delete team: active members: {names}"));
        }
        let outcome = team_file::cleanup_team_directories(&team_name)
            .map_err(|e| format!("Failed to delete team '{team_name}': {e}"))?;
        // Success path only: notify iff the task-list dir was removed.
        if outcome.tasks_dir_removed
            && let Some(notifier) = notifier
        {
            notifier.notify_change().await;
        }
        *self.active_team.write().await = None;
        Ok(DeleteTeamResult {
            team_name: Some(team_name),
            deleted: true,
        })
    }
}

fn unique_team_name(requested_name: &str) -> String {
    let base = crate::types::sanitize_name(requested_name);
    if !team_file::get_team_dir(&base).exists() {
        return base;
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if !team_file::get_team_dir(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}
