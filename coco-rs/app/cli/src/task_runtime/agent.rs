//! Agent task lifecycle — registration, output drain, terminal
//! transitions, completion notifications.

use std::sync::Arc;

use async_trait::async_trait;
use coco_tasks::{
    NotificationKind, TaskCreateRequest, TaskNotification, TaskUsage as NotifTaskUsage,
    TeammateTaskCreateRequest, TeammateTaskUpdate as RuntimeTeammateTaskUpdate, TerminalStatus,
    Worktree as NotifWorktree,
};
use coco_tool_runtime::{
    AgentCompletionPayload, AgentRegistration, TaskHandle, TeammateTaskRegistration,
    TeammateTaskUpdate,
};
use coco_types::{TaskStatus, TaskType};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, trace, warn};

use super::TaskRuntime;
use super::timers::spawn_agent_auto_background_timer;
use crate::disk_task_output::DEFAULT_MAX_READ_BYTES;

struct AgentTaskRegistrationRequest<'a> {
    task_id: String,
    description: &'a str,
    tool_use_id: Option<&'a str>,
    invoking_agent: Option<&'a str>,
    cancel: CancellationToken,
    auto_background_ms: Option<u64>,
    is_backgrounded: bool,
}

impl TaskRuntime {
    /// Register a freshly-spawned AgentTool task. The `registration`
    /// parameter selects the initial fg/bg state and the optional
    /// auto-detach timer (see [`AgentRegistration`]).
    ///
    /// Mints the id, resolves the disk path, inserts as `Running` with
    /// one lifecycle event, and stores per-task control state (cancel
    /// token + watch + invoking agent id). On
    /// `ForegroundWithAutoDetach`, spawns the per-task auto-detach
    /// timer in `timers::spawn_agent_auto_background_timer`.
    #[instrument(
        level = "info",
        skip(self, cancel),
        fields(description = %description, tool_use_id = ?tool_use_id, invoking_agent = ?invoking_agent, ?registration)
    )]
    pub async fn register_agent_task(
        &self,
        description: &str,
        tool_use_id: Option<&str>,
        invoking_agent: Option<&str>,
        cancel: CancellationToken,
        registration: AgentRegistration,
    ) -> String {
        let task_id = coco_types::generate_task_id(TaskType::BgAgent);
        self.register_agent_task_with_id(
            task_id,
            description,
            tool_use_id,
            invoking_agent,
            cancel,
            registration,
        )
        .await
    }

    pub async fn register_agent_task_with_id(
        &self,
        task_id: String,
        description: &str,
        tool_use_id: Option<&str>,
        invoking_agent: Option<&str>,
        cancel: CancellationToken,
        registration: AgentRegistration,
    ) -> String {
        let (is_backgrounded, auto_background_ms) = registration.decompose();
        self.register_agent_task_inner(AgentTaskRegistrationRequest {
            task_id,
            description,
            tool_use_id,
            invoking_agent,
            cancel,
            auto_background_ms,
            is_backgrounded,
        })
        .await
    }

    /// Internal helper that owns the actual registration steps. The
    /// public entry point [`Self::register_agent_task`] decomposes
    /// the `AgentRegistration` enum into the two underlying axes
    /// before calling this.
    async fn register_agent_task_inner(&self, request: AgentTaskRegistrationRequest<'_>) -> String {
        let AgentTaskRegistrationRequest {
            task_id,
            description,
            tool_use_id,
            invoking_agent,
            cancel,
            auto_background_ms,
            is_backgrounded,
        } = request;
        let dto = self.disk.get_or_create(&task_id).await;
        let output_path = dto.path().display().to_string();
        let assigned = self
            .manager
            .create_task(TaskCreateRequest {
                task_id: task_id.clone(),
                task_type: TaskType::BgAgent,
                description: description.to_string(),
                output_file: Some(output_path.clone()),
                tool_use_id: tool_use_id.map(String::from),
                is_backgrounded,
                status: TaskStatus::Running,
                cancel: cancel.clone(),
                invoking_agent: invoking_agent.map(String::from),
                shell_extras: None,
            })
            .await;
        debug_assert_eq!(assigned, task_id);
        // NOTE: no agent stall watchdog. Agents have no stdin and never emit
        // interactive prompts, so a silence-based stall notice only ever
        // misfired the shell-shaped "waiting for interactive input / re-run
        // with piped input" advice. The shell stall watchdog remains.
        // When `autoBackgroundMs` is set, the foreground sub-agent
        // auto-detaches after that many ms of execution. The fg awaiter
        // (`tool.execute`'s `select!` arm) gets the detach signal and
        // unblocks with `AsyncLaunched`; the engine keeps running detached.
        if let Some(ms) = auto_background_ms.filter(|v| *v > 0) {
            spawn_agent_auto_background_timer(task_id.clone(), ms, self.manager.clone(), cancel);
        }
        info!(
            target: "coco::task_runtime",
            task_id = %task_id,
            task_type = "local_agent",
            output_file = %output_path,
            auto_background_ms = ?auto_background_ms,
            "agent task registered (Running)"
        );
        task_id
    }

    /// Append text to a task's on-disk output file. Returns
    /// immediately — the actual fs write runs on the per-task drain
    /// task. Past the 5 GB cap, drops chunks and appends a single
    /// truncation marker.
    pub async fn append_output(&self, task_id: &str, chunk: &str) {
        let dto = self.disk.get_or_create(task_id).await;
        dto.append(chunk);
        trace!(
            target: "coco::task_runtime",
            task_id,
            chunk_bytes = chunk.len(),
            "appended chunk"
        );
    }

    /// Mark an agent task completed. Cancels the per-task token so
    /// periodic timers exit immediately, broadcasts the terminal
    /// status, and pushes a rich `<task-notification>` with optional
    /// `<result>` / `<usage>` / `<worktree>` sections.
    #[instrument(
        level = "info",
        skip(self, payload),
        fields(task_id = %task_id)
    )]
    pub async fn mark_completed(&self, task_id: &str, payload: AgentCompletionPayload) {
        if let Some(text) = payload.result.as_deref()
            && !text.is_empty()
        {
            self.append_output(task_id, text).await;
        }
        // Stamp authoritative final tokens + cost onto the progress slot
        // BEFORE the terminal transition so `emit_task_completed`'s
        // snapshot carries them to the TUI subagent panel.
        if let Some(u) = &payload.usage {
            self.manager
                .record_terminal_usage(task_id, u.usage, u.tool_uses, u.cost_usd)
                .await;
        }
        self.transition_terminal(task_id, TaskStatus::Completed)
            .await;
        self.push_agent_notification(task_id, TerminalStatus::Completed, payload, None)
            .await;
        info!(target: "coco::task_runtime", task_id, "task marked Completed");
    }

    /// Mark an agent task failed. Appends the error to the output
    /// buffer, flips status to `Failed`, fires the watch, and pushes
    /// a notification carrying the error in the summary.
    #[instrument(
        level = "info",
        skip(self, error),
        fields(task_id = %task_id, error_bytes = error.len())
    )]
    pub async fn mark_failed(&self, task_id: &str, error: &str) {
        self.append_output(task_id, error).await;
        // Record the error text on the sidecar so the post-compact
        // `task_status` reminder can surface it as `delta_summary` for
        // failed terminal tasks.
        self.manager.set_error(task_id, error.to_string()).await;
        self.transition_terminal(task_id, TaskStatus::Failed).await;
        self.push_agent_notification(
            task_id,
            TerminalStatus::Failed,
            AgentCompletionPayload::default(),
            Some(error.to_string()),
        )
        .await;
        warn!(target: "coco::task_runtime", task_id, "task marked Failed");
    }

    pub(super) async fn transition_terminal(&self, task_id: &str, status: TaskStatus) {
        debug_assert!(status.is_terminal());
        let _ = self.manager.transition_terminal(task_id, status).await;
    }

    /// Pull description, tool_use_id, output_file, and routing
    /// `invoking_agent` from the canonical TaskManager row; then
    /// push the agent-shaped notification.
    async fn push_agent_notification(
        &self,
        task_id: &str,
        status: TerminalStatus,
        payload: AgentCompletionPayload,
        error: Option<String>,
    ) {
        let Some(state) = self.manager.get(task_id).await else {
            return;
        };
        if !self.manager.mark_notified_once(task_id).await {
            return;
        }
        let invoking_agent = self.manager.invoking_agent(task_id).await;
        let n = TaskNotification {
            task_id: state.id,
            tool_use_id: state.tool_use_id,
            agent_id: invoking_agent,
            output_file: state.output_file.unwrap_or_default(),
            description: state.description,
            kind: NotificationKind::AgentTerminal {
                status,
                result: payload.result,
                usage: payload.usage.map(|u| NotifTaskUsage {
                    total_tokens: u.usage.input_tokens.total + u.usage.output_tokens.total,
                    tool_uses: u.tool_uses,
                    duration_ms: u.duration_ms,
                }),
                worktree: payload.worktree.map(|w| NotifWorktree {
                    path: w.path,
                    branch: w.branch,
                }),
                error,
            },
        };
        self.notification_sink.push(n).await;
    }
}

#[async_trait]
impl TaskHandle for TaskRuntime {
    // ── Reader (was TaskReader) ──
    async fn get_task_status(
        &self,
        task_id: &str,
    ) -> Result<coco_types::TaskStateBase, coco_error::BoxedError> {
        self.get_task_status_impl(task_id).await
    }
    async fn get_task_output_delta(
        &self,
        task_id: &str,
        from_offset: i64,
    ) -> Result<coco_tool_runtime::TaskOutputDelta, coco_error::BoxedError> {
        self.get_task_output_delta_impl(task_id, from_offset).await
    }
    async fn list_tasks(&self) -> Vec<coco_types::TaskStateBase> {
        self.list_tasks_impl().await
    }
    async fn subscribe_terminal(&self, task_id: &str) -> Option<coco_tool_runtime::TerminalSignal> {
        self.subscribe_terminal_impl(task_id).await
    }
    async fn detach_handle(&self, task_id: &str) -> Option<Arc<Notify>> {
        self.detach_handle_impl(task_id).await
    }
    async fn read_terminal_outputs(
        &self,
        task_id: &str,
    ) -> Result<coco_tool_runtime::TerminalOutputs, coco_error::BoxedError> {
        self.read_terminal_outputs_impl(task_id).await
    }
    async fn read_output(&self, task_id: &str) -> String {
        let Some(dto) = self.disk.get(task_id).await else {
            return String::new();
        };
        let _ = dto.flush().await;
        dto.read_tail(DEFAULT_MAX_READ_BYTES)
            .await
            .unwrap_or_default()
    }
    async fn task_state(&self, task_id: &str) -> Option<coco_types::TaskStateBase> {
        self.manager.get(task_id).await
    }
    async fn output_file_path(&self, task_id: &str) -> Option<std::path::PathBuf> {
        Some(self.disk.output_path(task_id))
    }
    async fn is_terminal(&self, task_id: &str) -> bool {
        self.manager
            .get(task_id)
            .await
            .map(|s| s.status.is_terminal())
            .unwrap_or(false)
    }

    // ── Controller (was TaskController) ──
    async fn kill_task(&self, task_id: &str) -> Result<(), coco_error::BoxedError> {
        self.kill_task_impl(task_id).await
    }
    async fn signal_detach(&self, task_id: &str) -> coco_tool_runtime::DetachOutcome {
        TaskRuntime::signal_detach(self, task_id).await
    }

    // ── Shell spawn (was ShellTaskSpawner) ──
    async fn spawn_shell_task(
        &self,
        request: coco_tool_runtime::BackgroundShellRequest,
    ) -> Result<String, coco_error::BoxedError> {
        self.spawn_shell_task_impl(request).await
    }

    // ── Agent task registration (was AgentTaskRegistry) ──
    async fn register_agent_task(
        &self,
        description: &str,
        tool_use_id: Option<&str>,
        invoking_agent: Option<&str>,
        cancel: CancellationToken,
        registration: AgentRegistration,
    ) -> String {
        TaskRuntime::register_agent_task(
            self,
            description,
            tool_use_id,
            invoking_agent,
            cancel,
            registration,
        )
        .await
    }

    async fn register_agent_task_with_id(
        &self,
        task_id: String,
        description: &str,
        tool_use_id: Option<&str>,
        invoking_agent: Option<&str>,
        cancel: CancellationToken,
        registration: AgentRegistration,
    ) -> String {
        TaskRuntime::register_agent_task_with_id(
            self,
            task_id,
            description,
            tool_use_id,
            invoking_agent,
            cancel,
            registration,
        )
        .await
    }
    async fn append_output(&self, task_id: &str, chunk: &str) {
        TaskRuntime::append_output(self, task_id, chunk).await
    }
    async fn set_progress_summary(&self, task_id: &str, summary: String) {
        self.manager.set_progress_summary(task_id, summary).await;
    }
    async fn set_progress(&self, task_id: &str, progress: coco_types::TaskProgress) {
        self.manager.set_progress(task_id, progress).await;
    }
    async fn mark_completed(&self, task_id: &str, payload: AgentCompletionPayload) {
        TaskRuntime::mark_completed(self, task_id, payload).await
    }
    async fn mark_failed(&self, task_id: &str, error: &str) {
        TaskRuntime::mark_failed(self, task_id, error).await
    }
    async fn complete_silent(&self, task_id: &str, succeeded: bool) {
        let status = if succeeded {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed
        };
        // Sync agent path writes terminal state but does NOT delete the
        // row — the panel-grace eviction sweep decides when the row goes
        // away. The previous behavior was to eagerly remove here, which
        // dropped the row before the 30s panel grace.
        let _ = self.manager.transition_terminal(task_id, status).await;
        info!(
            target: "coco::task_runtime",
            task_id,
            ?status,
            "complete_silent: terminal transition without notification (W4 sync path)"
        );
    }

    async fn register_dream_task(&self, description: &str, cancel: CancellationToken) -> String {
        let task_id = coco_types::generate_task_id(TaskType::Dream);
        let dto = self.disk.get_or_create(&task_id).await;
        let output_path = dto.path().display().to_string();
        let assigned = self
            .manager
            .create_task(TaskCreateRequest {
                task_id: task_id.clone(),
                task_type: TaskType::Dream,
                description: description.to_string(),
                output_file: Some(output_path.clone()),
                tool_use_id: None,
                is_backgrounded: true,
                status: TaskStatus::Running,
                cancel,
                invoking_agent: None,
                shell_extras: None,
            })
            .await;
        debug_assert_eq!(assigned, task_id);
        info!(
            target: "coco::task_runtime",
            task_id = %task_id,
            task_type = "dream",
            output_file = %output_path,
            "auto-dream task registered (Running)"
        );
        task_id
    }
    async fn register_teammate_task(&self, request: TeammateTaskRegistration) -> String {
        let task_id = coco_types::generate_task_id(TaskType::Teammate);
        let dto = self.disk.get_or_create(&task_id).await;
        let output_path = dto.path().display().to_string();
        let assigned = self
            .manager
            .create_teammate_task(TeammateTaskCreateRequest {
                task_id: task_id.clone(),
                agent_ref: request.agent_ref,
                backend_type: request.backend_type,
                pane_id: request.pane_id,
                prompt: request.prompt,
                output_file: Some(output_path.clone()),
                cancel: request.cancel,
            })
            .await;
        debug_assert_eq!(assigned, task_id);
        info!(
            target: "coco::task_runtime",
            task_id = %task_id,
            task_type = "in_process_teammate",
            output_file = %output_path,
            "teammate task registered (Running)"
        );
        task_id
    }
    async fn teammate_task_state(&self, agent_id: &str) -> Option<coco_types::TaskStateBase> {
        self.manager.find_teammate(agent_id).await
    }
    async fn update_teammate_task(&self, agent_id: &str, update: TeammateTaskUpdate) {
        self.manager
            .update_teammate_task(
                agent_id,
                RuntimeTeammateTaskUpdate {
                    is_idle: update.is_idle,
                    shutdown_requested: update.shutdown_requested,
                    result: update.result,
                    error: update.error,
                    spinner_verb: update.spinner_verb,
                    past_tense_verb: update.past_tense_verb,
                    append_message: update.append_message,
                },
            )
            .await;
    }
    async fn set_teammate_current_work_cancel(
        &self,
        agent_id: &str,
        cancel: Option<CancellationToken>,
    ) -> bool {
        self.manager
            .set_teammate_current_work_cancel(agent_id, cancel)
            .await
    }
    async fn interrupt_teammate_current_work(&self, agent_id: &str) -> Result<bool, String> {
        self.manager.interrupt_teammate_current_work(agent_id).await
    }
    async fn complete_teammate_task(
        &self,
        agent_id: &str,
        status: TaskStatus,
        result: Option<String>,
        error: Option<String>,
    ) {
        debug_assert!(status.is_terminal());
        let Some(state) = self.manager.find_teammate(agent_id).await else {
            return;
        };
        if let Some(text) = result.as_deref().filter(|s| !s.is_empty()) {
            self.append_output(&state.id, text).await;
        }
        if let Some(text) = error.as_deref().filter(|s| !s.is_empty()) {
            self.append_output(&state.id, text).await;
        }
        let result_update = match result {
            Some(text) => coco_types::FieldUpdate::Set(text),
            None => coco_types::FieldUpdate::Keep,
        };
        let error_update = match error {
            Some(text) => coco_types::FieldUpdate::Set(text),
            None => coco_types::FieldUpdate::Keep,
        };
        self.manager
            .update_teammate_task(
                agent_id,
                RuntimeTeammateTaskUpdate {
                    is_idle: coco_types::FieldUpdate::Set(true),
                    result: result_update,
                    error: error_update,
                    spinner_verb: coco_types::FieldUpdate::Clear,
                    ..RuntimeTeammateTaskUpdate::default()
                },
            )
            .await;
        let _ = self.manager.transition_terminal(&state.id, status).await;
        let _ = self.manager.mark_notified_once(&state.id).await;
    }
}
