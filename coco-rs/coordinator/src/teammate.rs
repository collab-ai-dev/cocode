//! Teammate lifecycle — model fallback, init hooks, mode snapshot,
//! leader permission bridge, and spawn orchestration helpers.

use std::sync::RwLock;

use crate::config::TeammateMode;

// ── Teammate Model Fallback (teammateModel.ts) ──

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTeammateModel {
    pub model: String,
    pub model_role: Option<coco_types::ModelRole>,
    pub model_selection: coco_types::LlmModelSelection,
}

/// Resolve the model for a teammate.
///
/// Priority: input `"inherit"` → leader; input string → explicit;
/// config concrete default → explicit; per-agent-type role; default
/// role; finally `ModelRole::Main`.
pub fn resolve_teammate_model(
    input_model: Option<&str>,
    leader_model: &str,
    config: &coco_config::AgentTeamsConfig,
    agent_type: Option<&str>,
    model_for_role: impl Fn(coco_types::ModelRole) -> Option<String>,
) -> ResolvedTeammateModel {
    if input_model == Some("inherit") {
        return ResolvedTeammateModel {
            model: leader_model.to_string(),
            model_role: Some(coco_types::ModelRole::Main),
            model_selection: coco_types::LlmModelSelection::InheritMain,
        };
    }

    if let Some(model) = input_model {
        let model_selection = coco_types::LlmModelSelection::from_model_and_role(Some(model), None);
        return ResolvedTeammateModel {
            model: model.to_string(),
            model_role: model_selection.fallback_role(),
            model_selection,
        };
    }

    if let Some(model) = config.default_model.as_ref() {
        let primary = coco_types::ProviderModelSelection {
            provider: model.provider.clone(),
            model_id: model.model_id.clone(),
        };
        return ResolvedTeammateModel {
            model: model.model_id.clone(),
            model_role: None,
            model_selection: coco_types::LlmModelSelection::Explicit { primary },
        };
    }

    let role = agent_type
        .and_then(|agent_type| config.agent_type_model_roles.get(agent_type).copied())
        .unwrap_or(config.default_model_role);
    ResolvedTeammateModel {
        model: model_for_role(role).unwrap_or_else(|| leader_model.to_string()),
        model_role: Some(role),
        model_selection: coco_types::LlmModelSelection::Role { role },
    }
}

// ── Teammate Mode Snapshot (teammateModeSnapshot.ts) ──

/// Captured teammate mode snapshot (frozen for session duration).
static MODE_SNAPSHOT: RwLock<Option<TeammateMode>> = RwLock::new(None);
/// CLI override for teammate mode.
static CLI_MODE_OVERRIDE: RwLock<Option<TeammateMode>> = RwLock::new(None);

/// Set a CLI override for teammate mode.
pub fn set_cli_teammate_mode_override(mode: TeammateMode) {
    if let Ok(mut guard) = CLI_MODE_OVERRIDE.write() {
        *guard = Some(mode);
    }
}

/// Get the CLI override for teammate mode.
pub fn get_cli_teammate_mode_override() -> Option<TeammateMode> {
    CLI_MODE_OVERRIDE.read().ok().and_then(|g| *g)
}

/// Capture the teammate mode at startup (frozen for session).
pub fn capture_teammate_mode_snapshot(config_mode: TeammateMode) {
    let mode = get_cli_teammate_mode_override().unwrap_or(config_mode);
    if let Ok(mut guard) = MODE_SNAPSHOT.write() {
        *guard = Some(mode);
    }
}

/// Get the captured teammate mode.
pub fn get_teammate_mode_from_snapshot() -> TeammateMode {
    MODE_SNAPSHOT
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or(TeammateMode::Auto)
}

// ── Leader Permission Bridge (leaderPermissionBridge.ts) ──

// Stores callbacks for in-process teammates to access the leader's
// ToolUseConfirm dialog, modeled as trait objects in a global registry.

use std::sync::Arc;
use tokio::sync::Mutex;

/// Callback for the leader's permission UI queue.
pub type PermissionQueueSetter = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

static LEADER_PERMISSION_QUEUE: Mutex<Option<PermissionQueueSetter>> = Mutex::const_new(None);

/// Register the leader's tool use confirm queue setter.
pub async fn register_leader_permission_queue(setter: PermissionQueueSetter) {
    *LEADER_PERMISSION_QUEUE.lock().await = Some(setter);
}

/// Get the registered leader permission queue setter.
pub async fn get_leader_permission_queue() -> Option<PermissionQueueSetter> {
    LEADER_PERMISSION_QUEUE.lock().await.clone()
}

/// Unregister the leader's permission queue.
pub async fn unregister_leader_permission_queue() {
    *LEADER_PERMISSION_QUEUE.lock().await = None;
}

// ── Spawn Orchestration Helpers (spawnMultiAgent.ts) ──

/// Configuration for spawning a teammate via the Agent tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnTeammateConfig {
    pub name: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub plan_mode_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Result of spawning a teammate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnOutput {
    pub teammate_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub tmux_session_name: String,
    pub tmux_window_name: String,
    pub tmux_pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    #[serde(default)]
    pub plan_mode_required: bool,
}

/// Generate a unique teammate name (avoids collisions with -2, -3 suffixes).
pub fn generate_unique_teammate_name(base_name: &str, existing_names: &[String]) -> String {
    if !existing_names.contains(&base_name.to_string()) {
        return base_name.to_string();
    }

    for suffix in 2..100 {
        let candidate = format!("{base_name}-{suffix}");
        if !existing_names.contains(&candidate) {
            return candidate;
        }
    }

    // Fallback: append UUID fragment
    format!(
        "{base_name}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Format a message as a teammate XML message.
pub fn format_as_teammate_message(
    from: &str,
    content: &str,
    color: Option<&str>,
    summary: Option<&str>,
) -> String {
    let color_attr = color.map(|c| format!(" color=\"{c}\"")).unwrap_or_default();
    let summary_attr = summary
        .map(|s| format!(" summary=\"{s}\""))
        .unwrap_or_default();
    format!(
        "<teammate_message teammate_id=\"{from}\"{color_attr}{summary_attr}>\n{content}\n</teammate_message>"
    )
}

// ── Teammate Init Hooks (teammateInit.ts) ──

/// Send an idle notification on teammate stop.
///
/// Sends an idle notification to the leader and marks the teammate as inactive
/// in the team file.
pub fn on_teammate_stop(
    agent_name: &str,
    team_name: &str,
    color: Option<&str>,
    last_summary: Option<&str>,
) {
    // Send idle notification to leader
    let idle_text =
        crate::mailbox::create_idle_notification(agent_name, Some("available"), last_summary);

    let message = crate::mailbox::TeammateMessage {
        from: agent_name.to_string(),
        text: idle_text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: color.map(String::from),
        summary: Some("idle notification".to_string()),
    };

    let _ = crate::mailbox::write_to_mailbox(crate::constants::TEAM_LEAD_NAME, message, team_name);

    // The runner-loop owns active/idle mutation through TeamRosterStore.
}

// ── Message Priority (inProcessRunner.ts) ──

/// Priority order for waiting for next prompt/shutdown.
///
/// Poll priority:
/// 1. In-memory pending user messages (highest)
/// 2. Abort signal check
/// 3. Mailbox messages:
///    a. Shutdown requests (highest mailbox priority)
///    b. Team-lead messages (second — leader = user intent)
///    c. Peer messages (FIFO, third)
/// 4. Unclaimed tasks from task list (lowest)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// Pending user messages from transcript view.
    PendingUserMessage = 0,
    /// Shutdown request from leader.
    ShutdownRequest = 1,
    /// Message from team lead.
    LeaderMessage = 2,
    /// Message from a peer teammate.
    PeerMessage = 3,
    /// Unclaimed task from task list.
    UnclaimedTask = 4,
}

#[cfg(test)]
#[path = "teammate.test.rs"]
mod tests;
