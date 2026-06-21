//! Inter-agent IPC message types: mailbox protocol used by leader and
//! teammates, plus per-subagent execution snapshots stored in `AppState`.
//!
//! Lives in `coco-types` (not `app/state`) so the future `coco-coordinator`
//! crate can read and write these without depending on `coco-state`. The
//! types are pure data with serde — no tokio, no app/state, no LLM.

use serde::Deserialize;
use serde::Serialize;

use crate::{PermissionMode, ProviderApi, WireApi};

/// Spawn-time snapshot of the parent's resolved provider+model identity.
///
/// Carried on `AgentSpawnRequest` / `AgentQueryConfig` (in-process; not
/// serialized at the JSON boundary) so the child's runner can:
///
/// - **Detect drift after hot-reload.** The parent ran on one runtime
///   snapshot; if a settings change publishes a new `Arc<RuntimeConfig>`
///   between the parent's last turn and the child's first turn, the
///   child's freshly resolved runtime may target a
///   different model/provider. Comparing the child's resolved identity
///   against this snapshot surfaces the drift.
///
/// - **Enforce Fork-mode cache parity.** Fork mode (TS
///   `forkSubagent.ts`) requires the child to send a byte-identical
///   request prefix to share the prompt cache. That requires identical
///   `(provider, api, model_id, base_url, wire_api)` between parent and
///   child. Mismatch → cache miss; the runner can warn or fail-loud.
///
/// **Why a thin DTO and not `ProviderClientFingerprint`?** The
/// fingerprint type lives in `coco-inference` and carries SHA-256
/// digests over `ProviderClientOptions` and the resolved API key. The
/// digests are computed from `coco-config` types via length-prefixed
/// hashing — that machinery is intentionally local to the inference
/// crate. This DTO captures the **identity-distinguishing** subset that
/// crosses layer boundaries (it lives in `coco-types`, the foundation
/// crate). When a runner wants the full digest comparison it builds the
/// `ProviderClientFingerprint` from its current runtime and asserts the
/// non-secret fields match this snapshot.
///
/// **No TS counterpart.** TS doesn't have hot-reloadable runtime config,
/// so identity-drift detection is a Rust-extension concern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubagentRuntimeSnapshot {
    /// Provider instance identifier (`ProviderConfig.name`). Distinct
    /// from `ProviderApi` — a single API can host multiple instances
    /// (`anthropic-prod`, `anthropic-dev`, `openrouter-anthropic`).
    pub provider: String,
    /// Wire-protocol family — `Anthropic`, `Openai`, `Gemini`, etc.
    pub api: ProviderApi,
    /// Resolved per-(provider, model) `api_model_name` — the literal
    /// string sent on the wire, after any provider-specific overrides.
    pub api_model_name: String,
    /// Endpoint base URL. Discriminates self-hosted / region / proxy
    /// configurations of the same provider.
    pub base_url: String,
    /// OpenAI-only `Chat` vs `Responses` discriminator. `None` for
    /// every other API family (the field is inert there and excluded
    /// from identity by construction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<WireApi>,
}

// `TeamContext` / `TeammateEntry` (the old `AppState.teamContext` mirror)
// were removed: coco-rs keeps live team authority in the coordinator roster,
// so these structs had no production reader — only an always-`None` parameter
// on `identity::get_team_name`, now dropped.

/// Standalone agent context (non-team agent identity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandaloneAgentContext {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Task entry in the AppState task map. Kept here (not in `coco-tasks`)
/// because it's the AppState-side projection used by the coordinator
/// runner loop's task scheduler — not the durable task-list shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskEntry {
    pub subject: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
}

/// Sub-agent execution status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
}

/// Per-subagent state snapshot shown in the AppState `sub_agents` map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentState {
    pub agent_id: String,
    pub name: String,
    pub status: SubAgentStatus,
    /// Number of turns completed.
    #[serde(default)]
    pub turns: i32,
    /// Model used by this agent.
    #[serde(default)]
    pub model: Option<String>,
    /// Working directory for this agent.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Last message from this agent.
    #[serde(default)]
    pub last_message: Option<String>,
}

/// Reason why an agent became idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleReason {
    Available,
    Interrupted,
    Failed,
}

/// Inter-agent envelope for the mailbox system. Renamed from the original
/// `AppState`-local `AgentMessage` to avoid collision with the unrelated
/// [`crate::ThreadItemDetails::AgentMessage`] streaming-content variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateProtocolMessage {
    pub from_agent: String,
    pub to_agent: String,
    pub content: TeammateProtocolContent,
    pub timestamp: i64,
}

/// Content of an inter-agent message. TS: `teammateMailbox.ts` defines
/// 15+ structured protocol message types — variants here are byte-faithful.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TeammateProtocolContent {
    /// Text message from one agent to another.
    Text { text: String },

    // ── Permission ──
    /// Permission request forwarded from sub-agent to leader.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        tool_use_id: String,
        description: String,
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        permission_suggestions: Vec<serde_json::Value>,
    },
    /// Permission response from leader to sub-agent.
    PermissionResponse {
        request_id: String,
        #[serde(default)]
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
    },

    // ── Sandbox Permission ──
    /// Sandbox permission request from worker to leader.
    SandboxPermissionRequest {
        request_id: String,
        worker_id: String,
        worker_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_color: Option<String>,
        host: String,
        created_at: i64,
    },
    /// Sandbox permission response from leader to worker.
    SandboxPermissionResponse {
        request_id: String,
        host: String,
        allow: bool,
    },

    // ── Task lifecycle ──
    /// Agent completed its task.
    TaskComplete { result: String },
    /// Agent encountered an error.
    TaskError { error: String },
    /// Task assignment from leader to teammate.
    TaskAssignment {
        task_id: String,
        subject: String,
        description: String,
        assigned_by: String,
    },

    // ── Idle notification ──
    /// Agent has become idle.
    IdleNotification {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idle_reason: Option<IdleReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed_status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure_reason: Option<String>,
    },

    // ── Shutdown lifecycle ──
    /// Request to shut down a teammate.
    ShutdownRequest {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Shutdown approved by teammate.
    ShutdownApproved {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backend_type: Option<String>,
    },
    /// Shutdown rejected by teammate.
    ShutdownRejected { request_id: String, reason: String },

    // ── Plan approval ──
    /// Plan approval request from teammate to leader.
    PlanApprovalRequest {
        request_id: String,
        plan_file_path: String,
        plan_content: String,
    },
    /// Plan approval response from leader to teammate.
    PlanApprovalResponse {
        request_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<PermissionMode>,
    },

    // ── Mode & permission updates ──
    /// Request to change permission mode.
    ModeSetRequest { mode: PermissionMode },
    /// Team-wide permission update.
    TeamPermissionUpdate {
        tool_name: String,
        directory_path: String,
        #[serde(default)]
        rules: Vec<serde_json::Value>,
        #[serde(default)]
        behavior: String,
    },
}

#[cfg(test)]
#[path = "agent_ipc.test.rs"]
mod tests;
