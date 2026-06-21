//! Background-task callback trait — the seam between tools and the
//! task subsystem.
//!
//!
//! ## One trait, one Arc
//!
//! [`TaskHandle`] is a single trait spanning read / control / spawn /
//! teammate-registry methods. Test mocks override only what they
//! exercise (every method has a default no-op / error implementation);
//! the production impl (`coco_cli::task_runtime::TaskRuntime`)
//! overrides everything.
//!
//! Phase-3a follow-up: split into `TaskReader` / `TaskLifecycle` /
//! `ShellSpawner` (+ a coordinator-owned `TeammateRegistry`). Tracked
//! but deferred — the renames in this pass already touch every
//! consumer call site, so the split is sequenced after.
//!
//! ## Output / lifecycle / notification: where each concern lives
//!
//! - Status + per-task state — [`coco_types::TaskStateBase`] (the
//!   canonical wire shape). [`TaskHandle::get_task_status`] returns it
//!   directly.
//! - On-disk output — `coco_cli::disk_task_output::DiskTaskOutput`.
//! - Notifications — `coco_tasks::notification::NotificationSink`.

use coco_messages::Message;
use coco_types::BackendType;
use coco_types::FieldUpdate;
use coco_types::TaskStateBase;
use coco_types::TeammateRef;
use std::path::PathBuf;
use std::sync::Arc;

/// Outcome of a [`TaskHandle::signal_detach`] call. Self-documents at
/// the callsite vs an opaque `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// First call — the per-task `detached` CAS flipped true and the
    /// fg awaiter `Notify` was woken. The task is now backgrounded.
    Detached,
    /// Second-or-later call — the task was already in the detached
    /// state. No-op (task is already backgrounded).
    AlreadyDetached,
    /// Task id unknown to the runtime — no per-task entry exists.
    Unknown,
}

impl DetachOutcome {
    /// True only on the first successful detach.
    pub fn is_first(self) -> bool {
        matches!(self, Self::Detached)
    }
}

/// Request to spawn a background shell task.
#[derive(Clone)]
pub struct BackgroundShellRequest {
    pub command: String,
    pub shell_kind: BackgroundShellKind,
    pub description: String,
    pub timeout_ms: Option<i64>,
    pub tool_use_id: Option<String>,
    /// Agent id of the subagent that issued the spawn. Routes the
    /// completion notification back to that agent's queue filter.
    /// Canonical format is the BgAgent id (`a<16hex>`).
    pub issuing_agent: Option<String>,
    /// When set, the TaskRuntime stdout-drain coroutine emits
    /// `bash_progress` events through this channel every
    /// `progress_throttle_ms`.
    pub progress_tx: Option<crate::traits::ProgressSender>,
    pub progress_throttle_ms: u64,
    /// When set, the TaskRuntime fires `signal_detach(task_id)`
    /// internally after this many ms of foreground execution.
    pub auto_detach_ms: Option<u64>,
    /// Whether the driver kills the child when it exceeds `timeout_ms`.
    /// `false` (auto-backgroundable foreground commands) means the timeout
    /// does NOT kill — paired with `auto_detach_ms = timeout_ms`, the fg
    /// awaiter is released and the child keeps running in the background until
    /// natural exit (`shouldAutoBackground`). `true` keeps the
    /// hard-kill-on-timeout behaviour (explicit bg, `sleep`, subagents).
    pub kill_on_timeout: bool,
    /// Optional sandbox runtime state.
    pub sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
    pub sandbox_bypass: coco_sandbox::SandboxBypass,
}

#[derive(Clone)]
pub enum BackgroundShellKind {
    DefaultPlatformShell,
    Provider(Arc<dyn coco_shell::ShellProvider>),
}

impl std::fmt::Debug for BackgroundShellKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefaultPlatformShell => f.write_str("DefaultPlatformShell"),
            Self::Provider(provider) => f
                .debug_tuple("Provider")
                .field(&provider.shell_type().name())
                .finish(),
        }
    }
}

impl std::fmt::Debug for BackgroundShellRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundShellRequest")
            .field("command", &self.command)
            .field("shell_kind", &self.shell_kind)
            .field("description", &self.description)
            .field("timeout_ms", &self.timeout_ms)
            .field("tool_use_id", &self.tool_use_id)
            .field("issuing_agent", &self.issuing_agent)
            .field("progress_tx", &self.progress_tx.is_some())
            .field("progress_throttle_ms", &self.progress_throttle_ms)
            .field("auto_detach_ms", &self.auto_detach_ms)
            .field("kill_on_timeout", &self.kill_on_timeout)
            .finish()
    }
}

/// Delta output from a background task (incremental read).
#[derive(Debug, Clone)]
pub struct TaskOutputDelta {
    pub content: String,
    pub new_offset: i64,
    pub is_complete: bool,
}

/// Terminal-state outputs for foreground `tool.execute` callers that
/// raced the task's terminal signal.
#[derive(Debug, Clone)]
pub struct TerminalOutputs {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}

/// One-shot terminal signal. Backed by `tokio::sync::watch::Receiver`.
pub struct TerminalSignal {
    rx: tokio::sync::watch::Receiver<coco_types::TaskStatus>,
}

impl TerminalSignal {
    pub fn new(rx: tokio::sync::watch::Receiver<coco_types::TaskStatus>) -> Self {
        Self { rx }
    }

    pub async fn await_terminal(mut self) -> coco_types::TaskStatus {
        if let Ok(borrow) = self.rx.wait_for(|s| s.is_terminal()).await {
            return *borrow;
        }
        *self.rx.borrow()
    }
}

/// Registration mode for an AgentTool spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRegistration {
    Foreground,
    ForegroundWithAutoDetach { ms: u64 },
    Background,
}

impl AgentRegistration {
    pub fn decompose(self) -> (bool, Option<u64>) {
        match self {
            Self::Foreground => (false, None),
            Self::ForegroundWithAutoDetach { ms } => (false, Some(ms)),
            Self::Background => (true, None),
        }
    }
}

/// Registration request for an in-process teammate execution row.
#[derive(Debug, Clone)]
pub struct TeammateTaskRegistration {
    /// `name@team` identity. The triplet `agent_id`/`agent_name`/
    /// `team_name` collapsed into this one parsed pair.
    pub agent_ref: TeammateRef,
    pub backend_type: BackendType,
    pub pane_id: Option<String>,
    pub prompt: String,
    pub cancel: tokio_util::sync::CancellationToken,
}

impl TeammateTaskRegistration {
    /// Convenience for callers that hold separate name + team strings.
    pub fn new(
        name: impl Into<String>,
        team: impl Into<String>,
        backend_type: BackendType,
        pane_id: Option<String>,
        prompt: String,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            agent_ref: TeammateRef::new(name, team),
            backend_type,
            pane_id,
            prompt,
            cancel,
        }
    }
}

/// Progress/update payload for a teammate task row. Uniform
/// [`FieldUpdate`] across all fields — booleans use `apply_required`
/// (`Clear` resets to `false`); `Option<String>` slots use `apply`.
#[derive(Debug, Clone, Default)]
pub struct TeammateTaskUpdate {
    pub is_idle: FieldUpdate<bool>,
    pub shutdown_requested: FieldUpdate<bool>,
    pub result: FieldUpdate<String>,
    pub error: FieldUpdate<String>,
    pub spinner_verb: FieldUpdate<String>,
    pub past_tense_verb: FieldUpdate<String>,
    pub append_message: Option<coco_types::TeammateTaskMessage>,
}

/// Optional payload threaded into the agent-task completion notification.
#[derive(Debug, Clone, Default)]
pub struct AgentCompletionPayload {
    pub result: Option<String>,
    pub usage: Option<AgentUsage>,
    pub worktree: Option<AgentWorktree>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentUsage {
    /// Full token breakdown (input/output + cache buckets). Reuses the
    /// engine's `TokenUsage` rather than carrying hand-picked scalars,
    /// so cache hits and every dimension flow through to the transcript
    /// summary.
    pub usage: coco_types::TokenUsage,
    pub tool_uses: i32,
    pub duration_ms: i64,
    /// Real USD cost of the completed subagent run. `0.0` when pricing
    /// is unavailable.
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct AgentWorktree {
    pub path: String,
    pub branch: Option<String>,
}

/// Sidecar metadata for a background AgentTool spawn.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentSpawnMetadata {
    pub agent_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Per-agent transcript persistence trait.
#[async_trait::async_trait]
pub trait AgentTranscriptStore: Send + Sync {
    async fn append_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
        messages: &[Arc<Message>],
    ) -> Result<(), coco_error::BoxedError>;

    async fn load_agent_messages(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<Vec<Arc<Message>>>, coco_error::BoxedError>;

    async fn write_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
        metadata: &AgentSpawnMetadata,
    ) -> Result<(), coco_error::BoxedError>;

    async fn read_agent_metadata(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<AgentSpawnMetadata>, coco_error::BoxedError>;
}

pub type AgentTranscriptStoreRef = Arc<dyn AgentTranscriptStore>;

/// Unified background-task seam — read, control, registration, shell
/// spawn, teammate registry. All consumers (engine, tools, coordinator)
/// hold the same `Arc<dyn TaskHandle>` and call the slice they need.
///
/// Every method has a default implementation that errors out / returns
/// empty, so test mocks only override the methods they actually exercise.
#[async_trait::async_trait]
pub trait TaskHandle: Send + Sync {
    // ── Read side ──

    async fn get_task_status(
        &self,
        _task_id: &str,
    ) -> Result<TaskStateBase, coco_error::BoxedError> {
        Err(unavail())
    }

    async fn get_task_output_delta(
        &self,
        _task_id: &str,
        _from_offset: i64,
    ) -> Result<TaskOutputDelta, coco_error::BoxedError> {
        Err(unavail())
    }

    async fn list_tasks(&self) -> Vec<TaskStateBase> {
        Vec::new()
    }

    async fn subscribe_terminal(&self, _task_id: &str) -> Option<TerminalSignal> {
        None
    }

    async fn detach_handle(&self, _task_id: &str) -> Option<Arc<tokio::sync::Notify>> {
        None
    }

    async fn read_terminal_outputs(
        &self,
        _task_id: &str,
    ) -> Result<TerminalOutputs, coco_error::BoxedError> {
        Err(unavail())
    }

    async fn read_output(&self, _task_id: &str) -> String {
        String::new()
    }

    async fn task_state(&self, _task_id: &str) -> Option<TaskStateBase> {
        None
    }

    async fn output_file_path(&self, _task_id: &str) -> Option<PathBuf> {
        None
    }

    async fn is_terminal(&self, _task_id: &str) -> bool {
        false
    }

    // ── Control side ──

    async fn kill_task(&self, _task_id: &str) -> Result<(), coco_error::BoxedError> {
        Err(unavail())
    }

    async fn signal_detach(&self, _task_id: &str) -> DetachOutcome {
        DetachOutcome::Unknown
    }

    // ── Shell-task spawn ──

    async fn spawn_shell_task(
        &self,
        _request: BackgroundShellRequest,
    ) -> Result<String, coco_error::BoxedError> {
        Err(unavail())
    }

    // ── BgAgent / Dream registration / progress ──

    async fn register_agent_task(
        &self,
        _description: &str,
        _tool_use_id: Option<&str>,
        _invoking_agent_id: Option<&str>,
        _cancel: tokio_util::sync::CancellationToken,
        _registration: AgentRegistration,
    ) -> String {
        String::new()
    }

    async fn register_agent_task_with_id(
        &self,
        task_id: String,
        _description: &str,
        _tool_use_id: Option<&str>,
        _invoking_agent_id: Option<&str>,
        _cancel: tokio_util::sync::CancellationToken,
        _registration: AgentRegistration,
    ) -> String {
        task_id
    }

    async fn register_dream_task(
        &self,
        _description: &str,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> String {
        String::new()
    }

    async fn append_output(&self, _task_id: &str, _chunk: &str) {}

    async fn set_progress_summary(&self, _task_id: &str, _summary: String) {}

    async fn set_progress(&self, _task_id: &str, _progress: coco_types::TaskProgress) {}

    async fn mark_completed(&self, _task_id: &str, _payload: AgentCompletionPayload) {}

    async fn mark_failed(&self, _task_id: &str, _error: &str) {}

    /// Sync-path terminal transition WITHOUT a `<task-notification>`
    /// envelope. Does **not** evict the row.
    async fn complete_silent(&self, _task_id: &str, _succeeded: bool) {}

    // ── Teammate registration / state (in-process) ──

    async fn register_teammate_task(&self, _request: TeammateTaskRegistration) -> String {
        String::new()
    }

    /// Teammate lookups accept the wire form (`"name@team"`) — implementations
    /// parse to [`TeammateRef`] internally so callers don't have to plumb the
    /// typed identity through every layer.
    async fn teammate_task_state(&self, _agent_id: &str) -> Option<TaskStateBase> {
        None
    }

    async fn update_teammate_task(&self, _agent_id: &str, _update: TeammateTaskUpdate) {}

    async fn set_teammate_current_work_cancel(
        &self,
        _agent_id: &str,
        _cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> bool {
        false
    }

    async fn interrupt_teammate_current_work(&self, agent_id: &str) -> Result<bool, String> {
        Err(format!("Teammate '{agent_id}' not found"))
    }

    async fn complete_teammate_task(
        &self,
        _agent_id: &str,
        _status: coco_types::TaskStatus,
        _result: Option<String>,
        _error: Option<String>,
    ) {
    }
}

pub type TaskHandleRef = Arc<dyn TaskHandle>;

// Aliases retained from the four-trait era for caller stability. The
// underlying type is the same `Arc<dyn TaskHandle>`.
pub type BackgroundTaskHandleRef = TaskHandleRef;
pub type AgentTaskRegistryRef = TaskHandleRef;

/// No-op implementation for contexts without background tasks.
#[derive(Debug, Clone, Default)]
pub struct NoOpBackgroundTaskHandle;

impl TaskHandle for NoOpBackgroundTaskHandle {}

fn unavail() -> coco_error::BoxedError {
    Box::new(coco_error::PlainError::new(
        "Background tasks not available in this context",
        coco_error::StatusCode::Internal,
    ))
}
