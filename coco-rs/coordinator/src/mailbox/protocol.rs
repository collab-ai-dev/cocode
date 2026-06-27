//! Structured protocol-message envelope used by teammates.
//!
//! Teammates exchange structured signals (idle, permission, plan
//! approval, shutdown, mode changes, …) through the same JSON inbox as
//! free-form messages. Each envelope is serialised as a one-line JSON
//! object whose `type` discriminator selects the variant; receivers
//! detect them via [`is_structured_protocol_message`] and decode via
//! [`parse_protocol_message`].
//!
//! On-disk permission directories (pending / resolved) and the
//! creator helpers for each variant also live here so the
//! protocol-shape definition and its emitters stay together.
//!
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use coco_types::BackendType;
use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;

use crate::team_file::get_team_dir;

use super::io::{TeammateMessage, write_to_mailbox};

// ── Protocol Message Helpers ──

/// Check if a message text is a structured protocol message.
pub fn is_structured_protocol_message(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    v.get("type").and_then(|t| t.as_str()).is_some_and(|t| {
        matches!(
            t,
            "idle_notification"
                | "permission_request"
                | "permission_response"
                | "sandbox_permission_request"
                | "sandbox_permission_response"
                | "plan_approval_request"
                | "plan_approval_response"
                | "shutdown_request"
                | "shutdown_approved"
                | "shutdown_rejected"
                | "task_assignment"
                | "team_permission_update"
                | "mode_set_request"
        )
    })
}

/// Parse a structured protocol message from text.
pub fn parse_protocol_message(text: &str) -> Option<ProtocolMessage> {
    let trimmed = text.trim();
    serde_json::from_str(trimmed).ok()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionResponsePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_updates: Vec<WirePermissionUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionResponseSubtype {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
}

impl PermissionResponseSubtype {
    pub const fn from_approved(approved: bool) -> Self {
        if approved { Self::Success } else { Self::Error }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

impl PermissionResponsePayload {
    pub fn into_permission_updates(self) -> Vec<PermissionUpdate> {
        self.permission_updates
            .into_iter()
            .map(WirePermissionUpdate::into_permission_update)
            .collect()
    }

    pub fn into_parts(self) -> (Option<serde_json::Value>, Vec<PermissionUpdate>) {
        (
            self.updated_input,
            self.permission_updates
                .into_iter()
                .map(WirePermissionUpdate::into_permission_update)
                .collect(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WireTeamPermissionUpdate {
    #[serde(rename = "addRules")]
    AddRules {
        rules: Vec<WirePermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: WireTeamPermissionUpdateDestination,
    },
}

impl WireTeamPermissionUpdate {
    pub fn into_permission_rules(self) -> Vec<PermissionRule> {
        match self {
            Self::AddRules {
                rules,
                behavior,
                destination: WireTeamPermissionUpdateDestination::Session,
            } => rules
                .into_iter()
                .map(|rule| rule.into_permission_rule(behavior))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WireTeamPermissionUpdateDestination {
    #[serde(rename = "session")]
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WirePermissionUpdate {
    #[serde(rename = "addRules")]
    AddRules {
        rules: Vec<WirePermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: WirePermissionUpdateDestination,
    },
    #[serde(rename = "replaceRules")]
    ReplaceRules {
        rules: Vec<WirePermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: WirePermissionUpdateDestination,
    },
    #[serde(rename = "removeRules")]
    RemoveRules {
        rules: Vec<WirePermissionRuleValue>,
        behavior: PermissionBehavior,
        destination: WirePermissionUpdateDestination,
    },
    #[serde(rename = "setMode")]
    SetMode {
        mode: PermissionMode,
        destination: WirePermissionUpdateDestination,
    },
    #[serde(rename = "addDirectories")]
    AddDirectories {
        directories: Vec<String>,
        destination: WirePermissionUpdateDestination,
    },
    #[serde(rename = "removeDirectories")]
    RemoveDirectories {
        directories: Vec<String>,
        destination: WirePermissionUpdateDestination,
    },
}

impl WirePermissionUpdate {
    pub fn into_permission_update(self) -> PermissionUpdate {
        match self {
            Self::AddRules {
                rules,
                behavior,
                destination,
            } => PermissionUpdate::AddRules {
                rules: rules
                    .into_iter()
                    .map(|rule| rule.into_permission_rule(behavior))
                    .collect(),
                destination: destination.into(),
            },
            Self::ReplaceRules {
                rules,
                behavior,
                destination,
            } => PermissionUpdate::ReplaceRules {
                rules: rules
                    .into_iter()
                    .map(|rule| rule.into_permission_rule(behavior))
                    .collect(),
                destination: destination.into(),
            },
            Self::RemoveRules {
                rules,
                behavior,
                destination,
            } => PermissionUpdate::RemoveRules {
                rules: rules
                    .into_iter()
                    .map(|rule| rule.into_permission_rule(behavior))
                    .collect(),
                destination: destination.into(),
            },
            Self::SetMode { mode, .. } => PermissionUpdate::SetMode { mode },
            Self::AddDirectories {
                directories,
                destination,
            } => PermissionUpdate::AddDirectories {
                directories,
                destination: destination.into(),
            },
            Self::RemoveDirectories {
                directories,
                destination,
            } => PermissionUpdate::RemoveDirectories {
                directories,
                destination: destination.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RuleUpdateKind {
    Add,
    Replace,
    Remove,
}

fn wire_permission_updates_from(update: PermissionUpdate) -> Vec<WirePermissionUpdate> {
    match update {
        PermissionUpdate::AddRules { rules, destination } => {
            wire_rule_updates(RuleUpdateKind::Add, rules, destination)
        }
        PermissionUpdate::ReplaceRules { rules, destination } => {
            wire_rule_updates(RuleUpdateKind::Replace, rules, destination)
        }
        PermissionUpdate::RemoveRules { rules, destination } => {
            wire_rule_updates(RuleUpdateKind::Remove, rules, destination)
        }
        PermissionUpdate::SetMode { mode } => vec![WirePermissionUpdate::SetMode {
            mode,
            destination: WirePermissionUpdateDestination::Session,
        }],
        PermissionUpdate::AddDirectories {
            directories,
            destination,
        } => vec![WirePermissionUpdate::AddDirectories {
            directories,
            destination: destination.into(),
        }],
        PermissionUpdate::RemoveDirectories {
            directories,
            destination,
        } => vec![WirePermissionUpdate::RemoveDirectories {
            directories,
            destination: destination.into(),
        }],
    }
}

fn wire_rule_updates(
    kind: RuleUpdateKind,
    rules: Vec<PermissionRule>,
    destination: PermissionUpdateDestination,
) -> Vec<WirePermissionUpdate> {
    if rules.is_empty() {
        return vec![wire_rule_update(
            kind,
            Vec::new(),
            PermissionBehavior::Allow,
            destination.into(),
        )];
    }

    let destination = WirePermissionUpdateDestination::from(destination);
    [
        PermissionBehavior::Allow,
        PermissionBehavior::Deny,
        PermissionBehavior::Ask,
    ]
    .into_iter()
    .filter_map(|behavior| {
        let rules = rules
            .iter()
            .filter(|rule| rule.behavior == behavior)
            .cloned()
            .map(WirePermissionRuleValue::from)
            .collect::<Vec<_>>();
        if rules.is_empty() {
            None
        } else {
            Some(wire_rule_update(kind, rules, behavior, destination))
        }
    })
    .collect()
}

fn wire_rule_update(
    kind: RuleUpdateKind,
    rules: Vec<WirePermissionRuleValue>,
    behavior: PermissionBehavior,
    destination: WirePermissionUpdateDestination,
) -> WirePermissionUpdate {
    match kind {
        RuleUpdateKind::Add => WirePermissionUpdate::AddRules {
            rules,
            behavior,
            destination,
        },
        RuleUpdateKind::Replace => WirePermissionUpdate::ReplaceRules {
            rules,
            behavior,
            destination,
        },
        RuleUpdateKind::Remove => WirePermissionUpdate::RemoveRules {
            rules,
            behavior,
            destination,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePermissionRuleValue {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

impl WirePermissionRuleValue {
    fn into_permission_rule(self, behavior: PermissionBehavior) -> PermissionRule {
        PermissionRule {
            source: PermissionRuleSource::Session,
            behavior,
            value: PermissionRuleValue {
                tool_pattern: self.tool_name,
                rule_content: self.rule_content,
            },
        }
    }
}

impl From<PermissionRule> for WirePermissionRuleValue {
    fn from(rule: PermissionRule) -> Self {
        Self {
            tool_name: rule.value.tool_pattern,
            rule_content: rule.value.rule_content,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WirePermissionUpdateDestination {
    #[serde(rename = "userSettings")]
    UserSettings,
    #[serde(rename = "projectSettings")]
    ProjectSettings,
    #[serde(rename = "localSettings")]
    LocalSettings,
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "cliArg")]
    CliArg,
    #[serde(rename = "command")]
    Command,
}

impl From<WirePermissionUpdateDestination> for PermissionUpdateDestination {
    fn from(destination: WirePermissionUpdateDestination) -> Self {
        match destination {
            WirePermissionUpdateDestination::UserSettings => Self::UserSettings,
            WirePermissionUpdateDestination::ProjectSettings => Self::ProjectSettings,
            WirePermissionUpdateDestination::LocalSettings => Self::LocalSettings,
            WirePermissionUpdateDestination::Session => Self::Session,
            WirePermissionUpdateDestination::CliArg => Self::CliArg,
            WirePermissionUpdateDestination::Command => Self::Command,
        }
    }
}

impl From<PermissionUpdateDestination> for WirePermissionUpdateDestination {
    fn from(destination: PermissionUpdateDestination) -> Self {
        match destination {
            PermissionUpdateDestination::UserSettings => Self::UserSettings,
            PermissionUpdateDestination::ProjectSettings => Self::ProjectSettings,
            PermissionUpdateDestination::LocalSettings => Self::LocalSettings,
            PermissionUpdateDestination::Session => Self::Session,
            PermissionUpdateDestination::CliArg => Self::CliArg,
            PermissionUpdateDestination::Command => Self::Command,
        }
    }
}

/// All structured protocol message types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProtocolMessage {
    IdleNotification {
        from: String,
        timestamp: String,
        #[serde(
            rename = "idleReason",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        idle_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(
            rename = "completedTaskId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        completed_task_id: Option<String>,
        #[serde(
            rename = "completedStatus",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        completed_status: Option<String>,
        #[serde(
            rename = "failureReason",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        failure_reason: Option<String>,
    },
    PermissionRequest {
        request_id: String,
        agent_id: String,
        tool_name: String,
        tool_use_id: String,
        description: String,
        #[serde(default)]
        input: serde_json::Value,
        /// Worker's tool execution cwd, so the leader resolves the worker's
        /// relative paths against the worker's directory (not the leader's)
        /// when deriving scoped grants.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        permission_suggestions: Vec<serde_json::Value>,
    },
    PermissionResponse {
        request_id: String,
        subtype: PermissionResponseSubtype,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<PermissionResponsePayload>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    SandboxPermissionRequest {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "workerId")]
        worker_id: String,
        #[serde(rename = "workerName")]
        worker_name: String,
        #[serde(
            rename = "workerColor",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        worker_color: Option<String>,
        #[serde(rename = "hostPattern")]
        host_pattern: serde_json::Value,
        #[serde(rename = "createdAt")]
        created_at: i64,
    },
    SandboxPermissionResponse {
        #[serde(rename = "requestId")]
        request_id: String,
        host: String,
        allow: bool,
        timestamp: String,
    },
    PlanApprovalRequest {
        from: String,
        timestamp: String,
        #[serde(rename = "planFilePath")]
        plan_file_path: String,
        #[serde(rename = "planContent")]
        plan_content: String,
        #[serde(rename = "requestId")]
        request_id: String,
    },
    PlanApprovalResponse {
        #[serde(rename = "requestId")]
        request_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
        // The leader-side writers (TUI human approve + model SendMessage)
        // serialize via `coco_tool_runtime::PlanApprovalResponse`, which has
        // no timestamp field. Without `default` the teammate's
        // `wait_for_plan_approval` parse fails and an actually-approving
        // leader blocks the teammate forever.
        #[serde(default)]
        timestamp: String,
        #[serde(
            rename = "permissionMode",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        permission_mode: Option<String>,
    },
    ShutdownRequest {
        #[serde(rename = "requestId")]
        request_id: String,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        timestamp: String,
    },
    ShutdownApproved {
        #[serde(rename = "requestId")]
        request_id: String,
        from: String,
        timestamp: String,
        #[serde(rename = "paneId", default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
        #[serde(
            rename = "backendType",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        backend_type: Option<BackendType>,
    },
    ShutdownRejected {
        #[serde(rename = "requestId")]
        request_id: String,
        from: String,
        reason: String,
        timestamp: String,
    },
    TaskAssignment {
        #[serde(rename = "taskId")]
        task_id: String,
        subject: String,
        description: String,
        #[serde(rename = "assignedBy")]
        assigned_by: String,
        timestamp: String,
    },
    TeamPermissionUpdate {
        #[serde(rename = "permissionUpdate")]
        permission_update: WireTeamPermissionUpdate,
        #[serde(rename = "directoryPath")]
        directory_path: String,
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    ModeSetRequest {
        mode: PermissionMode,
        from: String,
    },
}

/// Send a shutdown request to an agent's mailbox.
pub fn send_shutdown_request(
    target_name: &str,
    team_name: &str,
    from: &str,
    reason: Option<&str>,
) -> crate::Result<String> {
    let request_id = format!("shutdown-{}", uuid::Uuid::new_v4());
    let timestamp = chrono::Utc::now().to_rfc3339();

    let protocol = ProtocolMessage::ShutdownRequest {
        request_id: request_id.clone(),
        from: from.to_string(),
        reason: reason.map(String::from),
        timestamp: timestamp.clone(),
    };
    let text = serde_json::to_string(&protocol)?;

    let message = TeammateMessage {
        from: from.to_string(),
        text,
        timestamp,
        read: false,
        color: None,
        summary: Some("shutdown request".to_string()),
    };

    write_to_mailbox(target_name, message, team_name)?;
    Ok(request_id)
}

/// Create an idle notification protocol message.
pub fn create_idle_notification(
    from: &str,
    idle_reason: Option<&str>,
    summary: Option<&str>,
) -> String {
    let msg = ProtocolMessage::IdleNotification {
        from: from.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        idle_reason: idle_reason.map(String::from),
        summary: summary.map(String::from),
        completed_task_id: None,
        completed_status: None,
        failure_reason: None,
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Create a mode set request protocol message.
pub fn create_mode_set_request(mode: PermissionMode, from: &str) -> String {
    let msg = ProtocolMessage::ModeSetRequest {
        mode,
        from: from.to_string(),
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

// ── Permission Sync Directories ──

/// Get the permissions directory for a team.
pub fn permissions_dir(team_name: &str) -> PathBuf {
    get_team_dir(team_name).join("permissions")
}

/// Get the pending permissions directory.
pub fn pending_permissions_dir(team_name: &str) -> PathBuf {
    permissions_dir(team_name).join("pending")
}

/// Get the resolved permissions directory.
pub fn resolved_permissions_dir(team_name: &str) -> PathBuf {
    permissions_dir(team_name).join("resolved")
}

/// Ensure permission directories exist.
pub fn ensure_permission_dirs(team_name: &str) -> crate::Result<()> {
    std::fs::create_dir_all(pending_permissions_dir(team_name))?;
    std::fs::create_dir_all(resolved_permissions_dir(team_name))?;
    Ok(())
}

/// Write a permission request to the pending directory.
pub fn write_pending_permission(
    team_name: &str,
    request: &crate::types::SwarmPermissionRequest,
) -> crate::Result<()> {
    ensure_permission_dirs(team_name)?;
    let path = pending_permissions_dir(team_name).join(format!("{}.json", request.id));
    let content = serde_json::to_string_pretty(request)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Read and remove a resolved permission response.
pub fn read_resolved_permission(
    team_name: &str,
    request_id: &str,
) -> crate::Result<Option<serde_json::Value>> {
    let path = resolved_permissions_dir(team_name).join(format!("{request_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let value: serde_json::Value = serde_json::from_str(&content)?;
    std::fs::remove_file(&path)?;
    Ok(Some(value))
}

/// Read all pending permission requests.
pub fn read_pending_permissions(
    team_name: &str,
) -> crate::Result<Vec<crate::types::SwarmPermissionRequest>> {
    let dir = pending_permissions_dir(team_name);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut requests = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry.path().extension().is_some_and(|e| e == "json")
            && let Ok(content) = std::fs::read_to_string(entry.path())
            && let Ok(req) = serde_json::from_str(&content)
        {
            requests.push(req);
        }
    }
    Ok(requests)
}

/// Resolve a pending permission (move from pending/ to resolved/).
pub fn resolve_permission(
    team_name: &str,
    request_id: &str,
    resolution: &serde_json::Value,
) -> crate::Result<()> {
    ensure_permission_dirs(team_name)?;
    let pending = pending_permissions_dir(team_name).join(format!("{request_id}.json"));
    let resolved = resolved_permissions_dir(team_name).join(format!("{request_id}.json"));
    let content = serde_json::to_string_pretty(resolution)?;
    std::fs::write(&resolved, content)?;
    let _ = std::fs::remove_file(&pending);
    Ok(())
}

/// Clean up old resolved permissions.
pub fn cleanup_old_resolutions(team_name: &str, max_age_ms: i64) -> crate::Result<()> {
    let dir = resolved_permissions_dir(team_name);
    if !dir.is_dir() {
        return Ok(());
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
        {
            let mtime_ms = modified
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if now_ms - mtime_ms > max_age_ms {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Poll for a resolved permission response (non-blocking).
pub fn poll_for_response(
    team_name: &str,
    request_id: &str,
) -> crate::Result<Option<serde_json::Value>> {
    read_resolved_permission(team_name, request_id)
}

/// Remove a worker's resolved response file.
pub fn remove_worker_response(team_name: &str, request_id: &str) -> crate::Result<()> {
    let path = resolved_permissions_dir(team_name).join(format!("{request_id}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Send a permission request to the leader's mailbox.
pub fn send_permission_request_via_mailbox(
    request: &crate::types::SwarmPermissionRequest,
) -> crate::Result<()> {
    let text = serde_json::to_string(&ProtocolMessage::PermissionRequest {
        request_id: request.id.clone(),
        agent_id: request.worker_id.clone(),
        tool_name: request.tool_name.clone(),
        tool_use_id: request.tool_use_id.clone(),
        description: request.description.clone(),
        input: request.input.clone(),
        cwd: request.cwd.clone(),
        permission_suggestions: Vec::new(),
    })?;
    let message = TeammateMessage {
        from: request.worker_name.clone(),
        text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: request.worker_color.clone(),
        summary: Some("permission request".to_string()),
    };
    write_to_mailbox(
        crate::constants::TEAM_LEAD_NAME,
        message,
        &request.team_name,
    )
}

/// Send a permission response to a worker's mailbox.
pub fn send_permission_response_via_mailbox(
    worker_name: &str,
    request_id: &str,
    approved: bool,
    feedback: Option<&str>,
    updated_input: Option<serde_json::Value>,
    permission_updates: Vec<coco_types::PermissionUpdate>,
    team_name: &str,
) -> crate::Result<()> {
    let subtype = PermissionResponseSubtype::from_approved(approved);
    let response = permission_response_payload(updated_input, permission_updates);
    let text = serde_json::to_string(&ProtocolMessage::PermissionResponse {
        request_id: request_id.to_string(),
        subtype,
        response,
        error: if approved {
            None
        } else {
            feedback.map(String::from)
        },
    })?;
    let message = TeammateMessage {
        from: crate::constants::TEAM_LEAD_NAME.to_string(),
        text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: None,
        summary: Some("permission response".to_string()),
    };
    write_to_mailbox(worker_name, message, team_name)
}

/// Send a sandbox permission request to the leader's mailbox.
pub fn send_sandbox_permission_request_via_mailbox(
    worker_name: &str,
    worker_id: &str,
    request_id: &str,
    host: &str,
    team_name: &str,
) -> crate::Result<()> {
    let text = serde_json::to_string(&ProtocolMessage::SandboxPermissionRequest {
        request_id: request_id.to_string(),
        worker_id: worker_id.to_string(),
        worker_name: worker_name.to_string(),
        worker_color: None,
        host_pattern: serde_json::json!({"host": host}),
        created_at: chrono::Utc::now().timestamp_millis(),
    })?;
    let message = TeammateMessage {
        from: worker_name.to_string(),
        text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: None,
        summary: Some("sandbox permission request".to_string()),
    };
    write_to_mailbox(crate::constants::TEAM_LEAD_NAME, message, team_name)
}

/// Send a sandbox permission response to a worker's mailbox.
pub fn send_sandbox_permission_response_via_mailbox(
    worker_name: &str,
    request_id: &str,
    host: &str,
    allow: bool,
    team_name: &str,
) -> crate::Result<()> {
    let text = serde_json::to_string(&ProtocolMessage::SandboxPermissionResponse {
        request_id: request_id.to_string(),
        host: host.to_string(),
        allow,
        timestamp: chrono::Utc::now().to_rfc3339(),
    })?;
    let message = TeammateMessage {
        from: crate::constants::TEAM_LEAD_NAME.to_string(),
        text,
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
        color: None,
        summary: Some("sandbox permission response".to_string()),
    };
    write_to_mailbox(worker_name, message, team_name)
}

// ── Protocol Message Creators ──

/// Create a permission request protocol message string.
pub fn create_permission_request_message(
    request_id: &str,
    agent_id: &str,
    tool_name: &str,
    tool_use_id: &str,
    description: &str,
    input: &serde_json::Value,
    cwd: Option<&str>,
) -> String {
    let msg = ProtocolMessage::PermissionRequest {
        request_id: request_id.to_string(),
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        tool_use_id: tool_use_id.to_string(),
        description: description.to_string(),
        input: input.clone(),
        cwd: cwd.map(str::to_owned),
        permission_suggestions: Vec::new(),
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Create a permission response protocol message string.
pub fn create_permission_response_message(
    request_id: &str,
    approved: bool,
    feedback: Option<&str>,
) -> String {
    create_permission_response_message_with_payload(
        request_id,
        approved,
        feedback,
        None,
        Vec::new(),
    )
}

pub fn create_permission_response_message_with_payload(
    request_id: &str,
    approved: bool,
    feedback: Option<&str>,
    updated_input: Option<serde_json::Value>,
    permission_updates: Vec<coco_types::PermissionUpdate>,
) -> String {
    let msg = ProtocolMessage::PermissionResponse {
        request_id: request_id.to_string(),
        subtype: PermissionResponseSubtype::from_approved(approved),
        response: permission_response_payload(updated_input, permission_updates),
        error: if approved {
            None
        } else {
            feedback.map(String::from)
        },
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

fn permission_response_payload(
    updated_input: Option<serde_json::Value>,
    permission_updates: Vec<coco_types::PermissionUpdate>,
) -> Option<PermissionResponsePayload> {
    if updated_input.is_none() && permission_updates.is_empty() {
        return None;
    }
    Some(PermissionResponsePayload {
        updated_input,
        permission_updates: permission_updates
            .into_iter()
            .flat_map(wire_permission_updates_from)
            .collect(),
    })
}

/// Create a shutdown approved protocol message string.
///
/// `pane_id` / `backend_type` are the approving worker's OWN pane
/// coordinates (read from its `team.json` member entry). The leader's
/// inbox poller uses them to tear down the right pane: present ⇒ a
/// pane-based teammate the leader must `kill_pane`; absent (the
/// in-process case) ⇒ the teammate already exits via its runner-loop
/// break, so the leader only removes membership.
///
pub fn create_shutdown_approved_message(
    request_id: &str,
    from: &str,
    pane_id: Option<&str>,
    backend_type: Option<BackendType>,
) -> String {
    let msg = ProtocolMessage::ShutdownApproved {
        request_id: request_id.to_string(),
        from: from.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        pane_id: pane_id.filter(|s| !s.is_empty()).map(String::from),
        backend_type,
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Create a shutdown rejected protocol message string.
pub fn create_shutdown_rejected_message(request_id: &str, from: &str, reason: &str) -> String {
    let msg = ProtocolMessage::ShutdownRejected {
        request_id: request_id.to_string(),
        from: from.to_string(),
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Create a plan-approval request protocol message string.
///
/// Sent from a teammate to the leader after the teammate's plan-write
/// turn when `plan_mode_required` is set. The leader replies with a
/// matching [`ProtocolMessage::PlanApprovalResponse`] (request_id
/// echoed) which the runner picks up via
/// [`super::super::runner_loop::wait_for_plan_approval`].
///
pub fn create_plan_approval_request_message(
    from: &str,
    request_id: &str,
    plan_file_path: &str,
    plan_content: &str,
) -> String {
    let msg = ProtocolMessage::PlanApprovalRequest {
        from: from.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        plan_file_path: plan_file_path.to_string(),
        plan_content: plan_content.to_string(),
        request_id: request_id.to_string(),
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

// ── Protocol Message Type Checkers ──

/// Check if a message is a specific protocol type. Returns typed variant if match.
impl ProtocolMessage {
    /// Returns true if this message is an idle notification.
    pub fn is_idle_notification(&self) -> bool {
        matches!(self, Self::IdleNotification { .. })
    }
    /// Returns true if this message is a permission request.
    pub fn is_permission_request(&self) -> bool {
        matches!(self, Self::PermissionRequest { .. })
    }
    /// Returns true if this message is a permission response.
    pub fn is_permission_response(&self) -> bool {
        matches!(self, Self::PermissionResponse { .. })
    }
    /// Returns true if this message is a shutdown request.
    pub fn is_shutdown_request(&self) -> bool {
        matches!(self, Self::ShutdownRequest { .. })
    }
    /// Returns true if this message is a shutdown approval.
    pub fn is_shutdown_approved(&self) -> bool {
        matches!(self, Self::ShutdownApproved { .. })
    }
    /// Returns true if this message is a shutdown rejection.
    pub fn is_shutdown_rejected(&self) -> bool {
        matches!(self, Self::ShutdownRejected { .. })
    }
    /// Returns true if this message is a task assignment.
    pub fn is_task_assignment(&self) -> bool {
        matches!(self, Self::TaskAssignment { .. })
    }
    /// Returns true if this message is a team permission update.
    pub fn is_team_permission_update(&self) -> bool {
        matches!(self, Self::TeamPermissionUpdate { .. })
    }
    /// Returns true if this message is a mode set request.
    pub fn is_mode_set_request(&self) -> bool {
        matches!(self, Self::ModeSetRequest { .. })
    }
    /// Returns true if this message is a sandbox permission request.
    pub fn is_sandbox_permission_request(&self) -> bool {
        matches!(self, Self::SandboxPermissionRequest { .. })
    }
    /// Returns true if this message is a sandbox permission response.
    pub fn is_sandbox_permission_response(&self) -> bool {
        matches!(self, Self::SandboxPermissionResponse { .. })
    }
    /// Returns true if this message is a plan approval request.
    pub fn is_plan_approval_request(&self) -> bool {
        matches!(self, Self::PlanApprovalRequest { .. })
    }
    /// Returns true if this message is a plan approval response.
    pub fn is_plan_approval_response(&self) -> bool {
        matches!(self, Self::PlanApprovalResponse { .. })
    }
}

/// Parse a message text and check if it's a specific protocol type.
///
/// Convenience wrapper combining `parse_protocol_message` + type check.
pub fn check_message_type(text: &str, type_name: &str) -> Option<ProtocolMessage> {
    let msg = parse_protocol_message(text)?;
    let matches = match type_name {
        "idle_notification" => msg.is_idle_notification(),
        "permission_request" => msg.is_permission_request(),
        "permission_response" => msg.is_permission_response(),
        "shutdown_request" => msg.is_shutdown_request(),
        "shutdown_approved" => msg.is_shutdown_approved(),
        "shutdown_rejected" => msg.is_shutdown_rejected(),
        "task_assignment" => msg.is_task_assignment(),
        "team_permission_update" => msg.is_team_permission_update(),
        "mode_set_request" => msg.is_mode_set_request(),
        "sandbox_permission_request" => msg.is_sandbox_permission_request(),
        "sandbox_permission_response" => msg.is_sandbox_permission_response(),
        "plan_approval_request" => msg.is_plan_approval_request(),
        "plan_approval_response" => msg.is_plan_approval_response(),
        _ => false,
    };
    if matches { Some(msg) } else { None }
}

#[cfg(test)]
#[path = "protocol.test.rs"]
mod tests;
