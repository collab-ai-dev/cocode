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

/// Outcome of the implicit session-team bootstrap.
#[derive(Debug, Clone)]
pub enum InitializeSessionTeamResult {
    /// A team file was written and the session team was made active.
    Created { team_name: String },
    /// The team already existed on disk; reused without clobbering.
    AlreadyExists { team_name: String },
    /// The leader already had an active team; no-op.
    AlreadyActive { team_name: String },
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

    /// Bootstrap the implicit session team (`session-<id[:8]>`).
    ///
    /// Idempotent (mirrors the upstream read-existing short-circuit):
    /// - If the leader already has an active team, no-op.
    /// - If the team directory already exists on disk (resumed session),
    ///   reuse it without clobbering — read it back, set it active, and
    ///   re-route the team task-list.
    /// - Otherwise write the leader-only roster, set it active, and route
    ///   the team task-list.
    pub async fn initialize_session_team(
        &self,
        request: coco_tool_runtime::InitializeSessionTeamRequest,
    ) -> Result<InitializeSessionTeamResult, String> {
        if let Some(manager) = self.active_team.read().await.as_ref() {
            return Ok(InitializeSessionTeamResult::AlreadyActive {
                team_name: manager.team_name().to_string(),
            });
        }
        if request.leader_session_id.trim().is_empty() {
            return Err(
                "session-team bootstrap requires a non-empty leader session id".to_string(),
            );
        }

        let team_name = request.team_name;
        let task_list_id = crate::types::sanitize_name(&team_name);

        // Read-existing short-circuit: a resumed session re-bootstrapping
        // its team must not clobber a teammate's roster view.
        let existing = team_file::read_team_file(&team_name)
            .map_err(|e| format!("Failed to read team '{team_name}': {e}"))?;
        let (team_file, already_existed) = match existing {
            Some(tf) => (tf, true),
            None => {
                let tf = build_session_team_file(LeaderTeamSpec {
                    team_name: team_name.clone(),
                    description: None,
                    leader_session_id: request.leader_session_id,
                    leader_agent_type: request.leader_agent_type,
                    leader_model: request.leader_model,
                    cwd: request.cwd.display().to_string(),
                    allowed_paths: Vec::new(),
                });
                team_file::write_team_file(&team_name, &tf)
                    .map_err(|e| format!("Failed to create session team '{team_name}': {e}"))?;
                (tf, false)
            }
        };

        if let Some(router) = request.task_list_router
            && let Err(e) = router.route_team_task_list(&task_list_id).await
        {
            return Err(format!(
                "Failed to route task tools to team task list '{task_list_id}': {e}"
            ));
        }

        *self.active_team.write().await = Some(TeamManager::new(team_name.clone(), team_file));

        Ok(if already_existed {
            InitializeSessionTeamResult::AlreadyExists { team_name }
        } else {
            InitializeSessionTeamResult::Created { team_name }
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
            joined_at: chrono::Utc::now().timestamp_millis(),
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
}

/// Inputs for the deterministic leader-only [`TeamFile`] build used by
/// `initialize_session_team` (the implicit session-team bootstrap).
/// Carries only the leader's own coordinates — the sole member of a
/// freshly-created team is always the team lead.
struct LeaderTeamSpec {
    team_name: String,
    description: Option<String>,
    leader_session_id: String,
    leader_agent_type: Option<String>,
    leader_model: Option<String>,
    cwd: String,
    allowed_paths: Vec<TeamAllowedPath>,
}

/// Build the leader-only [`TeamFile`] (name `TEAM_LEAD_NAME`,
/// `backend_type` InProcess, `is_active` true) for the implicit
/// session-team bootstrap.
fn build_session_team_file(spec: LeaderTeamSpec) -> TeamFile {
    let lead_agent_id = format!("{TEAM_LEAD_NAME}@{}", spec.team_name);
    let now = chrono::Utc::now().timestamp_millis();
    TeamFile {
        name: spec.team_name,
        description: spec.description,
        created_at: now,
        lead_agent_id: lead_agent_id.clone(),
        lead_session_id: Some(spec.leader_session_id),
        hidden_pane_ids: Vec::new(),
        team_allowed_paths: spec.allowed_paths,
        members: vec![TeamMember {
            agent_id: lead_agent_id,
            name: TEAM_LEAD_NAME.to_string(),
            agent_type: Some(
                spec.leader_agent_type
                    .unwrap_or_else(|| "team-lead".to_string()),
            ),
            model: spec.leader_model,
            prompt: None,
            color: None,
            plan_mode_required: false,
            joined_at: now,
            tmux_pane_id: String::new(),
            cwd: spec.cwd,
            worktree_path: None,
            session_id: None,
            subscriptions: Vec::new(),
            backend_type: Some(BackendType::InProcess),
            is_active: true,
            mode: None,
        }],
    }
}

#[cfg(test)]
#[path = "roster_store.test.rs"]
mod tests;
