//! Running background tasks — backgrounded agents, shells, in-process
//! teammates (and the reserved remote-teammate slot), dream consolidation.
//!
//! ## Storage layout
//!
//! Two parallel maps keyed by task id (`String`):
//!
//! - `rows: HashMap<String, TaskStateBase>` — pure serializable wire
//!   data. This is what the SDK / transcript / introspection sees.
//! - `controls: HashMap<String, TaskControl>` — runtime-only handles
//!   (`CancellationToken`, `watch::Sender<TaskStatus>`, `Arc<Notify>`,
//!   `OnceLock<exit_code>`, optional teammate `current_work_cancel`,
//!   optional in-process teammate `JoinHandle`, optional agent liveness
//!   heartbeat). Never serialized; never leaked out as `Arc` shared refs —
//!   every mutator goes through `TaskManager`.
//!
//! Splitting the two halves keeps `TaskStateBase` a pure DTO: future
//! consumers (event hub, transcript JSONL) can clone the wire shape
//! without dragging cancel-token Arcs through them.

use coco_tool_runtime::AgentExecutionPhase;
use coco_tool_runtime::DetachOutcome;
use coco_tool_runtime::DetachSource;
use coco_types::BackendType;
use coco_types::CoreEvent;
use coco_types::FieldUpdate;
use coco_types::ServerNotification;
use coco_types::ShellExtras;
use coco_types::TaskCompletedParams;
use coco_types::TaskCompletionStatus;
use coco_types::TaskExtras;
use coco_types::TaskKilledBy;
use coco_types::TaskProgress;
use coco_types::TaskProgressParams;
use coco_types::TaskStartedParams;
use coco_types::TaskStateBase;
use coco_types::TaskStatus;
use coco_types::TaskType;
use coco_types::TaskUsage;
use coco_types::TeammateExtras;
use coco_types::TeammateRef;
use coco_types::TeammateTaskMessage;
use coco_types::WorkflowProgressEvent;

use crate::workflow_progress::apply_workflow_progress;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Grace period before the panel may evict a terminal BgAgent task.
pub const PANEL_GRACE_MS: i64 = 30_000;

/// Cap on the per-teammate UI message mirror.
const TEAMMATE_MESSAGES_UI_CAP: usize = 50;

/// Runtime-only control handles. Stored in a sibling map on
/// [`TaskManager`] and never serialized.
///
/// Phase-2 collapse: the in-process-teammate `JoinHandle` and the
/// per-turn cancel slot live here so a single keyspace owns every
/// runtime concern. The coordinator's `InProcessAgentRunner` no longer
/// holds a parallel `agents` map.
#[derive(Debug)]
struct TaskControl {
    cancel: CancellationToken,
    status_tx: watch::Sender<TaskStatus>,
    invoking_agent: Option<String>,
    detach: Arc<Notify>,
    detached: Arc<AtomicBool>,
    detach_source: Arc<OnceLock<DetachSource>>,
    exit_code: Arc<OnceLock<i32>>,
    /// In-process teammate turn cancel slot. Only populated for
    /// teammate rows; `None` everywhere else.
    current_work_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// In-process teammate runner-loop join handle. Owned here so
    /// the coordinator no longer keeps a parallel `agents` map.
    join_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Monotonic, runtime-only heartbeat for local background agents.
    /// The sender lives with the task control so terminal/removal lifecycle
    /// remains in the same keyspace as cancellation and status watches.
    agent_liveness_tx: Option<watch::Sender<AgentLivenessSnapshot>>,
}

impl TaskControl {
    fn new(
        cancel: CancellationToken,
        invoking_agent: Option<String>,
        initial_status: TaskStatus,
        track_agent_liveness: bool,
    ) -> Self {
        let (status_tx, _) = watch::channel(initial_status);
        let agent_liveness_tx = track_agent_liveness.then(|| {
            let (tx, _) = watch::channel(AgentLivenessSnapshot {
                sequence: 0,
                phase: AgentExecutionPhase::AwaitingModel,
                last_progress: tokio::time::Instant::now(),
            });
            tx
        });
        Self {
            cancel,
            status_tx,
            invoking_agent,
            detach: Arc::new(Notify::new()),
            detached: Arc::new(AtomicBool::new(false)),
            detach_source: Arc::new(OnceLock::new()),
            exit_code: Arc::new(OnceLock::new()),
            current_work_cancel: Arc::new(Mutex::new(None)),
            join_handle: Arc::new(Mutex::new(None)),
            agent_liveness_tx,
        }
    }
}

/// Runtime-only liveness state consumed by the agent-host watchdog.
#[derive(Debug, Clone, Copy)]
pub struct AgentLivenessSnapshot {
    pub sequence: u64,
    pub phase: AgentExecutionPhase,
    pub last_progress: tokio::time::Instant,
}

pub struct TaskManager {
    rows: Arc<RwLock<HashMap<String, TaskStateBase>>>,
    controls: Arc<RwLock<HashMap<String, TaskControl>>>,
    event_tx: Option<mpsc::Sender<CoreEvent>>,
    sdk_summaries_enabled: Option<Arc<AtomicBool>>,
    job_ledger: Option<Arc<JobLedger>>,
    /// Emit gates for `LocalWorkflow` progress, keyed by task id.
    /// See [`WORKFLOW_PROGRESS_COALESCE_MS`].
    workflow_emit: Arc<std::sync::Mutex<HashMap<String, WorkflowEmitGate>>>,
}

/// Minimum wall-clock gap between two `task/progress` frames for one workflow.
///
/// Every frame carries the run's **whole** progress array, so emitting one per
/// delta makes a run quadratic in its own delta count — a `log()` loop or a
/// wide fan-out pays it in full. Coalescing bounds the emit rate; the trailing
/// flush bounds the staleness, so no consumer is ever further behind than this.
const WORKFLOW_PROGRESS_COALESCE_MS: i64 = 16;

/// Per-workflow emit gate.
#[derive(Debug, Default)]
struct WorkflowEmitGate {
    /// When the last frame went out.
    last_emit_ms: i64,
    /// A trailing flush is already pending, so a suppressed delta does not need
    /// to schedule a second one — the pending flush reads the array fresh and
    /// therefore already carries it.
    flush_pending: bool,
}

/// Durable job-ledger binding for this manager's background tasks. When
/// installed, spawn and terminal transitions write [`crate::JobState`]
/// records under `<config_home>/bg-jobs/`, so `coco ps` reports real
/// terminal outcomes and a process restart does not silently lose them.
pub struct JobLedger {
    pub store: crate::JobStore,
    pub session_id: coco_types::SessionId,
    pub cwd: std::path::PathBuf,
    pub kind: coco_session::ProcessSessionKind,
}

pub struct TaskCreateRequest {
    pub task_id: String,
    pub task_type: TaskType,
    pub description: String,
    pub output_file: Option<String>,
    pub tool_use_id: Option<String>,
    pub is_backgrounded: bool,
    pub status: TaskStatus,
    pub cancel: CancellationToken,
    pub invoking_agent: Option<String>,
    /// Workflow run id (`wf_…`) for `TaskType::LocalWorkflow`. Empty for
    /// other task types.
    pub workflow_run_id: String,
    pub workflow_name: Option<String>,
    pub workflow_prompt: Option<String>,
    /// Pre-populated shell extras for `TaskType::Shell`. Ignored for
    /// other task types.
    pub shell_extras: Option<ShellExtras>,
}

pub struct TeammateTaskCreateRequest {
    pub task_id: String,
    pub agent_ref: TeammateRef,
    pub backend_type: BackendType,
    pub pane_id: Option<String>,
    pub prompt: String,
    pub output_file: Option<String>,
    pub cancel: CancellationToken,
}

/// Partial update payload for teammate rows. Uniform [`FieldUpdate`]
/// across all fields — booleans use [`FieldUpdate::apply_required`]
/// (so `Clear` sets to `false`), strings use [`FieldUpdate::apply`]
/// against `Option<String>` slots.
#[derive(Debug, Clone, Default)]
pub struct TeammateTaskUpdate {
    pub is_idle: FieldUpdate<bool>,
    pub shutdown_requested: FieldUpdate<bool>,
    pub result: FieldUpdate<String>,
    pub error: FieldUpdate<String>,
    pub spinner_verb: FieldUpdate<String>,
    pub past_tense_verb: FieldUpdate<String>,
    pub append_message: Option<TeammateTaskMessage>,
}

impl std::fmt::Debug for TaskManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskManager")
            .field("event_sink", &self.event_tx.is_some())
            .finish()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            rows: Arc::new(RwLock::new(HashMap::new())),
            controls: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sdk_summaries_enabled: None,
            job_ledger: None,
            workflow_emit: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Install the durable job ledger. Spawn/terminal transitions then
    /// write `bg-jobs/` records (best-effort — a write failure is logged
    /// and never blocks the task lifecycle).
    pub fn with_job_ledger(mut self, ledger: JobLedger) -> Self {
        self.job_ledger = Some(Arc::new(ledger));
        self
    }

    /// Best-effort durable job-record write, off the async path.
    fn write_job_record(
        &self,
        id: &str,
        status: TaskStatus,
        name: Option<String>,
        error: Option<String>,
        created_at: i64,
    ) {
        let Some(ledger) = self.job_ledger.clone() else {
            return;
        };
        let job = crate::JobState {
            id: id.to_string(),
            session_id: ledger.session_id.clone(),
            cwd: ledger.cwd.clone(),
            kind: ledger.kind,
            created_at,
            updated_at: crate::job_store::now_ms(),
            status,
            name,
            intent: None,
            error,
        };
        tokio::task::spawn_blocking(move || {
            if let Err(error) = ledger.store.write(&job) {
                tracing::warn!(job = %job.id, %error, "job-ledger write failed");
            }
        });
    }

    pub fn with_summary_emission_gate(mut self, flag: Arc<AtomicBool>) -> Self {
        self.sdk_summaries_enabled = Some(flag);
        self
    }

    pub fn with_event_sink(mut self, event_tx: mpsc::Sender<CoreEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    /// Update progress on any task that carries a progress slot
    /// (BgAgent / Teammate / RemoteTeammate). Preserves the existing
    /// `summary` so writes from token-counter timers don't clobber the
    /// AgentSummary text. Emits `TaskProgress` on actual change.
    pub async fn set_progress(&self, id: &str, mut progress: TaskProgress) {
        let mut emit_payload: Option<TaskProgress> = None;
        {
            let mut rows = self.rows.write().await;
            if let Some(row) = rows.get_mut(id) {
                let Some(slot) = row.extras.progress_slot_mut() else {
                    return;
                };
                if let Some(existing_summary) = slot
                    .as_ref()
                    .and_then(|p| p.summary.clone())
                    .filter(|_| progress.summary.is_none())
                {
                    progress.summary = Some(existing_summary);
                }
                if slot.as_ref() != Some(&progress) {
                    emit_payload = Some(progress.clone());
                    *slot = Some(progress);
                }
            }
        }
        if let Some(payload) = emit_payload {
            self.emit_progress(id, payload).await;
        }
    }

    pub async fn set_progress_summary(&self, id: &str, summary: String) {
        let mut emit_payload: Option<TaskProgress> = None;
        {
            let mut rows = self.rows.write().await;
            if let Some(row) = rows.get_mut(id) {
                let Some(slot) = row.extras.progress_slot_mut() else {
                    return;
                };
                let mut p = slot.clone().unwrap_or_default();
                if p.summary.as_deref() == Some(summary.as_str()) {
                    return;
                }
                p.summary = Some(summary);
                emit_payload = Some(p.clone());
                *slot = Some(p);
            }
        }
        if let Some(payload) = emit_payload {
            self.emit_progress(id, payload).await;
        }
    }

    /// Stamp authoritative terminal tokens + cost onto the progress slot
    /// immediately before a terminal transition. `cost_usd` is stored as
    /// micro-USD (integer, to keep `TaskProgress: Eq`). Emits nothing —
    /// the imminent `transition_terminal` → `emit_task_completed` reads
    /// the slot and forwards these to the TUI.
    pub async fn record_terminal_usage(
        &self,
        id: &str,
        usage: coco_types::TokenUsage,
        tool_uses: i32,
        cost_usd: f64,
        input_cost_usd: f64,
        output_cost_usd: f64,
    ) {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.get_mut(id) else { return };
        let Some(slot) = row.extras.progress_slot_mut() else {
            return;
        };
        let total = usage.input_tokens.total + usage.output_tokens.total;
        let progress = slot.get_or_insert_with(TaskProgress::default);
        progress.total_tokens = progress.total_tokens.max(total);
        progress.input_tokens = progress.input_tokens.max(usage.input_tokens.total);
        progress.output_tokens = progress.output_tokens.max(usage.output_tokens.total);
        progress.cache_read_tokens = progress
            .cache_read_tokens
            .max(usage.input_tokens.cache_read);
        progress.tool_use_count = progress.tool_use_count.max(tool_uses);
        progress.cost_micro_usd = (cost_usd * 1_000_000.0) as i64;
        progress.input_cost_micro_usd = (input_cost_usd * 1_000_000.0) as i64;
        progress.output_cost_micro_usd = (output_cost_usd * 1_000_000.0) as i64;
    }

    async fn emit_progress(&self, task_id: &str, progress: TaskProgress) {
        let Some(tx) = &self.event_tx else {
            tracing::trace!(
                task_id = %task_id,
                "emit_progress: no event sink wired; TaskProgress dropped"
            );
            return;
        };
        if let Some(gate) = &self.sdk_summaries_enabled
            && !gate.load(Ordering::Relaxed)
        {
            tracing::debug!(
                task_id = %task_id,
                "emit_progress: suppressed — sdk_summaries gate closed"
            );
            return;
        }
        let Some(state) = self.rows.read().await.get(task_id).cloned() else {
            tracing::debug!(
                task_id = %task_id,
                "emit_progress: task row missing; TaskProgress dropped"
            );
            return;
        };
        let duration_ms = current_time_ms().saturating_sub(state.start_time);
        let params = TaskProgressParams {
            task_id: task_id.to_string(),
            tool_use_id: state.tool_use_id,
            description: state.description,
            usage: TaskUsage {
                total_tokens: progress.total_tokens,
                input_tokens: progress.input_tokens,
                output_tokens: progress.output_tokens,
                cache_read_tokens: progress.cache_read_tokens,
                tool_uses: progress.tool_use_count,
                duration_ms,
                cost_usd: progress.cost_micro_usd as f64 / 1_000_000.0,
                input_cost_usd: progress.input_cost_micro_usd as f64 / 1_000_000.0,
                output_cost_usd: progress.output_cost_micro_usd as f64 / 1_000_000.0,
            },
            last_tool_name: progress.last_tool_name,
            summary: progress.summary,
            agent_type: progress.agent_type,
            recent_activities: progress.recent_activities,
            workflow_progress: Vec::new(),
        };
        let tool_uses = params.usage.tool_uses;
        match tx
            .send(CoreEvent::Protocol(ServerNotification::TaskProgress(
                params,
            )))
            .await
        {
            Ok(()) => tracing::debug!(
                task_id = %task_id,
                tool_uses,
                "emit_progress: TaskProgress sent to event sink"
            ),
            Err(e) => tracing::warn!(
                task_id = %task_id,
                error = %e,
                "emit_progress: TaskProgress send failed (receiver dropped)"
            ),
        }
    }

    pub async fn mark_retrieved(&self, id: &str) {
        if let Some(extras) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .and_then(|r| r.extras.bg_agent_mut())
        {
            extras.retrieved = true;
        }
    }

    pub async fn set_retain(&self, id: &str, retain: bool) {
        if let Some(extras) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .and_then(|r| r.extras.bg_agent_mut())
        {
            extras.retain = retain;
        }
    }

    pub async fn set_evict_after(&self, id: &str, evict_after_ms: Option<i64>) {
        if let Some(extras) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .and_then(|r| r.extras.bg_agent_mut())
        {
            extras.evict_after = evict_after_ms;
        }
    }

    pub async fn set_backgrounded(&self, id: &str, backgrounded: bool) -> bool {
        let Some(updated) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .map(|r| r.extras.set_backgrounded(backgrounded))
        else {
            return false;
        };
        updated
    }

    /// Flip `is_backgrounded` and wake the detach waiter on every non-terminal,
    /// non-already-backgrounded running task whose type supports backgrounding
    /// (BgAgent + Shell + LocalWorkflow).
    /// Returns the wire ids that were just transitioned. Emits no wire event
    /// — foreground→background is a pure UI-state transition, not a task
    /// lifecycle event (the task continues running and will emit its
    /// own `task/completed` with the `output_file` populated when it actually
    /// terminates). The TUI mirror in `session.subagents` flips to
    /// `Backgrounded` optimistically inside the keybinding handler before
    /// dispatching the `UserCommand::BackgroundAllTasks`; Shell rows likewise
    /// flip silently and surface via `is_backgrounded` at render time.
    ///
    /// Drives the user-initiated `Ctrl+B` single-press path (`task:background`
    /// → `UserCommand::BackgroundAllTasks`). Idempotent: a second call with
    /// no foreground tasks returns an empty Vec.
    pub async fn background_all_foreground(&self) -> Vec<String> {
        let mut transitions: Vec<String> = Vec::new();
        let mut notifications: Vec<Arc<Notify>> = Vec::new();
        let mut rows = self.rows.write().await;
        let controls = self.controls.read().await;
        for (id, row) in rows.iter_mut() {
            if row.status.is_terminal() || row.extras.is_backgrounded() {
                continue;
            }
            let task_type = row.extras.task_type();
            if !matches!(
                task_type,
                TaskType::BgAgent | TaskType::Shell | TaskType::LocalWorkflow
            ) {
                continue;
            }
            let Some(control) = controls.get(id) else {
                continue;
            };
            if control.detached.swap(true, Ordering::SeqCst) {
                continue;
            }
            let _ = control.detach_source.set(DetachSource::User);
            row.extras.set_backgrounded(true);
            transitions.push(id.clone());
            notifications.push(control.detach.clone());
        }
        drop(controls);
        drop(rows);
        for detach in notifications {
            detach.notify_one();
        }
        transitions
    }

    pub async fn set_error(&self, id: &str, error: String) {
        if let Some(extras) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .and_then(|r| r.extras.bg_agent_mut())
        {
            extras.error = Some(error);
        }
    }

    pub async fn create_task(&self, request: TaskCreateRequest) -> String {
        let mut extras = match request.task_type {
            TaskType::BgAgent => TaskExtras::bg_agent_default(),
            TaskType::Dream => TaskExtras::dream(),
            TaskType::Shell => match request.shell_extras {
                Some(shell) => TaskExtras::Shell(shell),
                None => TaskExtras::shell_default(),
            },
            TaskType::LocalWorkflow => TaskExtras::local_workflow(
                request.workflow_run_id.clone(),
                request.workflow_name.clone(),
                request.workflow_prompt,
            ),
            TaskType::Teammate => {
                panic!(
                    "create_task called with TaskType::Teammate — use create_teammate_task instead"
                );
            }
            TaskType::RemoteTeammate => {
                panic!(
                    "create_task called with TaskType::RemoteTeammate — no driver implemented yet"
                );
            }
        };
        extras.set_backgrounded(request.is_backgrounded);
        let id = request.task_id;
        let state = TaskStateBase {
            id: id.clone(),
            status: request.status,
            notified: false,
            description: request.description,
            tool_use_id: request.tool_use_id,
            start_time: current_time_ms(),
            end_time: None,
            killed_by: None,
            total_paused_ms: None,
            output_file: request.output_file,
            output_offset: 0,
            extras,
        };
        let control = TaskControl::new(
            request.cancel,
            request.invoking_agent,
            request.status,
            request.task_type == TaskType::BgAgent,
        );
        let emit_state = state.clone();
        {
            let mut rows = self.rows.write().await;
            let mut controls = self.controls.write().await;
            rows.insert(id.clone(), state);
            controls.insert(id.clone(), control);
        }
        // Durable spawn record: a restart mid-task must not silently lose
        // the job — `coco ps` reconciles a Running record with no live PID
        // into an observable stale state.
        self.write_job_record(
            &id,
            emit_state.status,
            Some(emit_state.description.clone()),
            /*error*/ None,
            emit_state.start_time,
        );
        self.emit_task_started(&emit_state).await;
        id
    }

    pub async fn create_teammate_task(&self, request: TeammateTaskCreateRequest) -> String {
        let id = request.task_id;
        let description = request.agent_ref.to_string();
        let mut extras =
            TeammateExtras::new(request.agent_ref, request.backend_type, request.prompt);
        extras.pane_id = request.pane_id;
        let state = TaskStateBase {
            id: id.clone(),
            status: TaskStatus::Running,
            notified: false,
            description,
            tool_use_id: None,
            start_time: current_time_ms(),
            end_time: None,
            killed_by: None,
            total_paused_ms: None,
            output_file: request.output_file,
            output_offset: 0,
            extras: TaskExtras::Teammate(extras),
        };
        let control = TaskControl::new(request.cancel, None, TaskStatus::Running, false);
        let emit_state = state.clone();
        {
            let mut rows = self.rows.write().await;
            let mut controls = self.controls.write().await;
            rows.insert(id.clone(), state);
            controls.insert(id.clone(), control);
        }
        // Durable spawn record: a restart mid-task must not silently lose
        // the job — `coco ps` reconciles a Running record with no live PID
        // into an observable stale state.
        self.write_job_record(
            &id,
            emit_state.status,
            Some(emit_state.description.clone()),
            /*error*/ None,
            emit_state.start_time,
        );
        self.emit_task_started(&emit_state).await;
        id
    }

    pub async fn get(&self, id: &str) -> Option<TaskStateBase> {
        self.rows.read().await.get(id).cloned()
    }

    /// Advance an agent heartbeat without touching its serialized task row.
    ///
    /// Called on every streamed delta, so it samples: a heartbeat that is
    /// already fresh and in the same phase is dropped without taking the
    /// controls lock twice or waking the watchdog. The watchdog's shortest
    /// limit is minutes, so sub-second resolution buys nothing.
    pub async fn record_agent_activity(&self, id: &str, phase: AgentExecutionPhase) {
        const HEARTBEAT_RESOLUTION: std::time::Duration = std::time::Duration::from_secs(1);

        let sender = self
            .controls
            .read()
            .await
            .get(id)
            .and_then(|control| control.agent_liveness_tx.clone());
        let Some(sender) = sender else {
            return;
        };
        let now = tokio::time::Instant::now();
        let current = *sender.borrow();
        if current.phase == phase
            && now.saturating_duration_since(current.last_progress) < HEARTBEAT_RESOLUTION
        {
            return;
        }
        sender.send_modify(|snapshot| {
            snapshot.sequence = snapshot.sequence.saturating_add(1);
            snapshot.phase = phase;
            snapshot.last_progress = now;
        });
    }

    /// Subscribe to the runtime-only heartbeat for a local background agent.
    pub async fn subscribe_agent_liveness(
        &self,
        id: &str,
    ) -> Option<watch::Receiver<AgentLivenessSnapshot>> {
        self.controls.read().await.get(id).and_then(|control| {
            control
                .agent_liveness_tx
                .as_ref()
                .map(watch::Sender::subscribe)
        })
    }

    /// Locate the live teammate row by its `name@team` identity.
    /// Accepts the wire form (string); returns the most-recent live
    /// row, falling back to terminal if no live row exists.
    pub async fn find_teammate(&self, agent_id: &str) -> Option<TaskStateBase> {
        let rows = self.rows.read().await;
        let mut matches = rows
            .values()
            .filter(|state| {
                state
                    .teammate_extras()
                    .is_some_and(|extras| extras.agent_ref.to_string() == agent_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        matches.sort_by_key(|state| {
            (
                state.status.is_terminal(),
                std::cmp::Reverse(state.start_time),
            )
        });
        matches.into_iter().next()
    }

    pub async fn update_teammate_task(&self, agent_id: &str, update: TeammateTaskUpdate) {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.values_mut().find(|state| {
            state
                .teammate_extras()
                .is_some_and(|extras| extras.agent_ref.to_string() == agent_id)
                && !state.status.is_terminal()
        }) else {
            return;
        };
        let Some(extras) = row.extras.teammate_mut() else {
            return;
        };
        update.is_idle.apply_required(&mut extras.is_idle);
        update
            .shutdown_requested
            .apply_required(&mut extras.shutdown_requested);
        update.result.apply(&mut extras.result);
        update.error.apply(&mut extras.error);
        update.spinner_verb.apply(&mut extras.spinner_verb);
        update.past_tense_verb.apply(&mut extras.past_tense_verb);
        if let Some(message) = update.append_message {
            extras.messages.push(message);
            if extras.messages.len() > TEAMMATE_MESSAGES_UI_CAP {
                let drain_count = extras.messages.len() - TEAMMATE_MESSAGES_UI_CAP;
                extras.messages.drain(..drain_count);
            }
        }
    }

    pub async fn enqueue_teammate_user_message(&self, agent_id: &str, message: String) {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.values_mut().find(|state| {
            state
                .teammate_extras()
                .is_some_and(|extras| extras.agent_ref.to_string() == agent_id)
                && !state.status.is_terminal()
        }) else {
            return;
        };
        let Some(extras) = row.extras.teammate_mut() else {
            return;
        };
        extras.pending_user_messages.push(message);
    }

    pub async fn drain_teammate_user_messages(&self, agent_id: &str) -> Vec<String> {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.values_mut().find(|state| {
            state
                .teammate_extras()
                .is_some_and(|extras| extras.agent_ref.to_string() == agent_id)
                && !state.status.is_terminal()
        }) else {
            return Vec::new();
        };
        let Some(extras) = row.extras.teammate_mut() else {
            return Vec::new();
        };
        std::mem::take(&mut extras.pending_user_messages)
    }

    pub async fn update_status(&self, id: &str, status: TaskStatus) {
        if status.is_terminal() {
            let _ = self.transition_terminal(id, status).await;
            return;
        }
        let snapshot = {
            let mut rows = self.rows.write().await;
            if let Some(row) = rows.get_mut(id) {
                row.status = status;
                Some(row.clone())
            } else {
                None
            }
        };
        if let Some(task) = snapshot {
            self.emit_task_progress(id, &task).await;
        }
    }

    pub async fn transition_terminal(&self, id: &str, status: TaskStatus) -> Option<TaskStateBase> {
        self.transition_terminal_with_actor(id, status, None).await
    }

    pub async fn transition_terminal_with_actor(
        &self,
        id: &str,
        status: TaskStatus,
        killed_by: Option<TaskKilledBy>,
    ) -> Option<TaskStateBase> {
        debug_assert!(status.is_terminal());
        let snapshot = {
            let mut rows = self.rows.write().await;
            let row = rows.get_mut(id)?;
            if row.status.is_terminal() {
                return None;
            }
            row.status = status;
            row.end_time = Some(current_time_ms());
            row.killed_by = if status == TaskStatus::Killed {
                killed_by.or(row.killed_by)
            } else {
                None
            };
            // Dream tasks have no model-facing `<task-notification>`
            // envelope (UI-only). Auto-mark notified so
            // `remove_completed` evicts them without waiting for a reader.
            //
            // Shell is intentionally NOT auto-notified here: the natural
            // completion path runs through `apply_shell_terminal_state`,
            // which itself claims the notification slot via
            // `mark_notified_once` to compose the model-visible
            // `<shell-terminal>` envelope. Pre-setting `notified` would
            // suppress that producer. The asymmetry with `kill_running`
            // is deliberate: kill_running runs ahead of the producer to
            // ensure the cancellation path skips the duplicate envelope.
            if matches!(row.task_type(), TaskType::Dream) {
                row.notified = true;
            }
            if matches!(row.extras.task_type(), TaskType::BgAgent)
                && let Some(extras) = row.extras.bg_agent_mut()
                && !extras.retain
            {
                extras.evict_after = Some(current_time_ms() + PANEL_GRACE_MS);
            }
            row.clone()
        };
        if let Some(control) = self.controls.read().await.get(id) {
            control.cancel.cancel();
            control.status_tx.send_replace(status);
        }
        // Durable terminal record — survives process exit so `coco ps`
        // reports real done/failed/stopped outcomes.
        self.write_job_record(
            id,
            status,
            Some(snapshot.description.clone()),
            /*error*/ None,
            snapshot.start_time,
        );
        // A coalesced progress delta may still be pending; deliver the final
        // array before the completion event so no consumer's last view of the
        // run is one frame stale.
        if matches!(snapshot.extras.task_type(), TaskType::LocalWorkflow) {
            self.settle_workflow_progress(id).await;
        }
        self.emit_task_completed(id, &snapshot).await;
        Some(snapshot)
    }

    pub async fn list(&self) -> Vec<TaskStateBase> {
        self.rows.read().await.values().cloned().collect()
    }

    pub async fn advance_output_offset_if_running(
        &self,
        task_id: &str,
        observed_offset: i64,
        new_offset: i64,
    ) -> bool {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.get_mut(task_id) else {
            return false;
        };
        if row.status != TaskStatus::Running || row.output_offset != observed_offset {
            return false;
        }
        row.output_offset = new_offset;
        true
    }

    pub async fn remove_completed(&self) -> usize {
        let now = current_time_ms();
        let removable: Vec<String> = {
            let rows = self.rows.read().await;
            rows.iter()
                .filter(|(_id, t)| {
                    if !t.status.is_terminal() {
                        return false;
                    }
                    if !t.notified {
                        return false;
                    }
                    if t.task_type() == TaskType::BgAgent
                        && let Some(extras) = t.bg_agent_extras()
                    {
                        if extras.retain {
                            return false;
                        }
                        if let Some(deadline) = extras.evict_after
                            && deadline > now
                        {
                            return false;
                        }
                    }
                    true
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let count = removable.len();
        if count > 0 {
            let mut rows = self.rows.write().await;
            let mut controls = self.controls.write().await;
            for id in &removable {
                rows.remove(id);
                controls.remove(id);
            }
        }
        count
    }

    pub async fn remove_task(&self, id: &str) -> bool {
        let removed = self.rows.write().await.remove(id).is_some();
        self.controls.write().await.remove(id);
        removed
    }

    pub async fn mark_notified_once(&self, id: &str) -> bool {
        let mut rows = self.rows.write().await;
        let Some(row) = rows.get_mut(id) else {
            return false;
        };
        if row.notified {
            return false;
        }
        row.notified = true;
        true
    }

    pub async fn kill_running(&self, id: &str) -> Result<(), KillTaskError> {
        self.kill_running_by(id, TaskKilledBy::User).await
    }

    pub async fn kill_running_by(
        &self,
        id: &str,
        killed_by: TaskKilledBy,
    ) -> Result<(), KillTaskError> {
        let cancel = {
            let mut rows = self.rows.write().await;
            let row = rows.get_mut(id).ok_or(KillTaskError::NotFound)?;
            if row.status.is_terminal() {
                return Err(KillTaskError::NotRunning);
            }
            row.killed_by = Some(killed_by);
            if matches!(row.task_type(), TaskType::Dream)
                || (row.task_type() == TaskType::Shell && killed_by != TaskKilledBy::System)
            {
                row.notified = true;
            }
            self.controls
                .read()
                .await
                .get(id)
                .map(|c| c.cancel.clone())
                .ok_or(KillTaskError::NotFound)?
        };
        cancel.cancel();
        Ok(())
    }

    pub async fn signal_detach(&self, id: &str) -> DetachOutcome {
        self.signal_detach_with_source(id, DetachSource::User).await
    }

    pub async fn signal_detach_with_source(&self, id: &str, source: DetachSource) -> DetachOutcome {
        let detach = {
            let mut rows = self.rows.write().await;
            let Some(row) = rows.get_mut(id) else {
                return DetachOutcome::Unknown;
            };
            if row.status.is_terminal() {
                return DetachOutcome::Unknown;
            }
            let Some((detach, detached, detach_source)) =
                self.controls.read().await.get(id).map(|c| {
                    (
                        c.detach.clone(),
                        c.detached.clone(),
                        c.detach_source.clone(),
                    )
                })
            else {
                return DetachOutcome::Unknown;
            };
            if detached.swap(true, Ordering::SeqCst) {
                return DetachOutcome::AlreadyDetached;
            }
            let _ = detach_source.set(source);
            row.extras.set_backgrounded(true);
            detach
        };
        detach.notify_one();
        DetachOutcome::Detached
    }

    pub async fn detach_source(&self, id: &str) -> Option<DetachSource> {
        self.controls
            .read()
            .await
            .get(id)
            .and_then(|c| c.detach_source.get().copied())
    }

    pub async fn subscribe_terminal(&self, id: &str) -> Option<watch::Receiver<TaskStatus>> {
        self.controls
            .read()
            .await
            .get(id)
            .map(|c| c.status_tx.subscribe())
    }

    pub async fn detach_handle(&self, id: &str) -> Option<Arc<Notify>> {
        self.controls.read().await.get(id).map(|c| c.detach.clone())
    }

    pub async fn invoking_agent(&self, id: &str) -> Option<String> {
        self.controls
            .read()
            .await
            .get(id)
            .and_then(|c| c.invoking_agent.clone())
    }

    pub async fn set_teammate_current_work_cancel(
        &self,
        agent_id: &str,
        cancel: Option<CancellationToken>,
    ) -> bool {
        let task_id = self.lookup_teammate_id(agent_id).await;
        let Some(task_id) = task_id else {
            return false;
        };
        let slot = self
            .controls
            .read()
            .await
            .get(&task_id)
            .map(|c| c.current_work_cancel.clone());
        let Some(slot) = slot else {
            return false;
        };
        *slot.lock().await = cancel;
        true
    }

    pub async fn interrupt_teammate_current_work(&self, agent_id: &str) -> Result<bool, String> {
        let task_id = self
            .lookup_teammate_id(agent_id)
            .await
            .ok_or_else(|| format!("Teammate '{agent_id}' not found"))?;
        let slot = self
            .controls
            .read()
            .await
            .get(&task_id)
            .map(|c| c.current_work_cancel.clone())
            .ok_or_else(|| format!("Teammate '{agent_id}' control entry missing"))?;
        let guard = slot.lock().await;
        let Some(cancel) = guard.as_ref() else {
            return Ok(false);
        };
        cancel.cancel();
        Ok(true)
    }

    /// Store the in-process teammate runner-loop `JoinHandle` on the
    /// task's control entry. Phase-2 collapse.
    pub async fn set_teammate_join_handle(&self, agent_id: &str, join: JoinHandle<()>) -> bool {
        let task_id = self.lookup_teammate_id(agent_id).await;
        let Some(task_id) = task_id else {
            return false;
        };
        let slot = self
            .controls
            .read()
            .await
            .get(&task_id)
            .map(|c| c.join_handle.clone());
        let Some(slot) = slot else {
            return false;
        };
        *slot.lock().await = Some(join);
        true
    }

    pub async fn take_teammate_join_handle(&self, agent_id: &str) -> Option<JoinHandle<()>> {
        let task_id = self.lookup_teammate_id(agent_id).await?;
        let slot = self
            .controls
            .read()
            .await
            .get(&task_id)
            .map(|c| c.join_handle.clone())?;
        slot.lock().await.take()
    }

    pub async fn cancel_token(&self, id: &str) -> Option<CancellationToken> {
        self.controls.read().await.get(id).map(|c| c.cancel.clone())
    }

    pub async fn set_exit_code(&self, id: &str, exit_code: i32) {
        if let Some(control) = self.controls.read().await.get(id) {
            let _ = control.exit_code.set(exit_code);
        }
        if let Some(extras) = self
            .rows
            .write()
            .await
            .get_mut(id)
            .and_then(|r| r.extras.shell_mut())
        {
            extras.exit_code = Some(exit_code);
        }
    }

    pub async fn exit_code(&self, id: &str) -> Option<i32> {
        self.controls
            .read()
            .await
            .get(id)
            .and_then(|c| c.exit_code.get().copied())
    }

    async fn lookup_teammate_id(&self, agent_id: &str) -> Option<String> {
        let rows = self.rows.read().await;
        rows.values()
            .find(|state| {
                state
                    .teammate_extras()
                    .is_some_and(|extras| extras.agent_ref.to_string() == agent_id)
                    && !state.status.is_terminal()
            })
            .map(|state| state.id.clone())
    }

    async fn emit_task_started(&self, state: &TaskStateBase) {
        let Some(tx) = &self.event_tx else { return };
        let workflow = match &state.extras {
            TaskExtras::LocalWorkflow(extras) => Some(extras),
            _ => None,
        };
        let params = TaskStartedParams {
            task_id: state.id.clone(),
            tool_use_id: state.tool_use_id.clone(),
            description: state.description.clone(),
            task_type: Some(task_type_wire_name(state.task_type()).to_string()),
            workflow_name: workflow.and_then(|extras| extras.workflow_name.clone()),
            prompt: workflow.and_then(|extras| extras.prompt.clone()),
            agent_name: None,
            team_name: None,
            color: None,
            backend_kind: None,
        };
        let _ = tx
            .send(CoreEvent::Protocol(ServerNotification::TaskStarted(params)))
            .await;
    }

    /// Fold a workflow progress delta into a `LocalWorkflow` row's
    /// `workflow_progress` array and publish a `task/progress` carrying the
    /// cumulative array. No-op for non-`LocalWorkflow` rows or unknown ids.
    /// (The generic `set_progress`/`emit_progress` paths intentionally drop
    /// workflow deltas — only the workflow frame carries `workflow_progress`.)
    ///
    /// The fold is an index-keyed upsert plus a log trim
    /// ([`apply_workflow_progress`]), so the array stays bounded and one
    /// `agent()` call stays one node however many frames it emits.
    ///
    /// **The fold always runs; only publishing is rate-limited.** The row is
    /// therefore always current, while frames — each carrying the whole array —
    /// are capped at one per [`WORKFLOW_PROGRESS_COALESCE_MS`]. Deltas inside
    /// that window ride a trailing flush, which re-reads the row and so carries
    /// everything accumulated, not just the delta that scheduled it.
    pub async fn push_workflow_progress(&self, id: &str, mut event: WorkflowProgressEvent) {
        crate::workflow_progress::stamp_progress_time(&mut event, current_time_ms());
        {
            let mut rows = self.rows.write().await;
            let Some(row) = rows.get_mut(id) else {
                return;
            };
            let TaskExtras::LocalWorkflow(extras) = &mut row.extras else {
                return;
            };
            apply_workflow_progress(&mut extras.workflow_progress, event);
        }
        // The fold above is unconditional — the row is always current. Only the
        // *publishing* is rate-limited.
        match self.claim_workflow_emit(id) {
            WorkflowEmitDecision::Now => self.emit_workflow_frame(id).await,
            WorkflowEmitDecision::After(delay) => {
                let rows = self.rows.clone();
                let event_tx = self.event_tx.clone();
                let gates = self.workflow_emit.clone();
                let id = id.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Ok(mut gates) = gates.lock()
                        && let Some(gate) = gates.get_mut(&id)
                    {
                        gate.flush_pending = false;
                        gate.last_emit_ms = current_time_ms();
                    }
                    emit_workflow_frame(&rows, event_tx.as_ref(), &id).await;
                });
            }
            WorkflowEmitDecision::Coalesced => {}
        }
    }

    /// Decide whether this delta publishes now, rides a pending flush, or
    /// schedules one. Updates the gate in the same critical section so two
    /// concurrent deltas cannot both schedule a flush.
    fn claim_workflow_emit(&self, task_id: &str) -> WorkflowEmitDecision {
        let now = current_time_ms();
        let Ok(mut gates) = self.workflow_emit.lock() else {
            // Poisoned by a panicking holder: publish rather than lose the
            // frame — a stale panel is worse than an extra event.
            return WorkflowEmitDecision::Now;
        };
        let gate = gates.entry(task_id.to_string()).or_default();
        let elapsed = now.saturating_sub(gate.last_emit_ms);
        if elapsed >= WORKFLOW_PROGRESS_COALESCE_MS {
            gate.last_emit_ms = now;
            return WorkflowEmitDecision::Now;
        }
        if gate.flush_pending {
            return WorkflowEmitDecision::Coalesced;
        }
        gate.flush_pending = true;
        WorkflowEmitDecision::After(std::time::Duration::from_millis(
            (WORKFLOW_PROGRESS_COALESCE_MS - elapsed).max(0) as u64,
        ))
    }

    /// Publish the run's current progress array unconditionally.
    async fn emit_workflow_frame(&self, task_id: &str) {
        emit_workflow_frame(&self.rows, self.event_tx.as_ref(), task_id).await;
    }

    /// Deliver the final progress array and retire the gate. Called on the
    /// terminal transition so a coalesced tail delta is never the frame a
    /// consumer is left holding.
    async fn settle_workflow_progress(&self, task_id: &str) {
        if let Ok(mut gates) = self.workflow_emit.lock() {
            gates.remove(task_id);
        }
        self.emit_workflow_frame(task_id).await;
    }

    async fn emit_task_progress(&self, task_id: &str, state: &TaskStateBase) {
        let Some(tx) = &self.event_tx else { return };
        let _ = tx
            .send(CoreEvent::Protocol(ServerNotification::TaskProgress(
                task_progress_params(task_id, state),
            )))
            .await;
    }

    async fn emit_task_completed(&self, task_id: &str, state: &TaskStateBase) {
        let Some(tx) = &self.event_tx else { return };
        let status = task_status_to_completion(state.status);
        let duration_ms = state
            .end_time
            .unwrap_or_else(current_time_ms)
            .saturating_sub(state.start_time);
        let output_file = state.output_file.clone().unwrap_or_default();
        // Final tokens + cost are stamped onto the progress slot by
        // `record_terminal_usage` just before the terminal transition,
        // so the snapshot carries authoritative values here.
        let progress = state.progress();
        let usage = TaskUsage {
            total_tokens: progress.map(|p| p.total_tokens).unwrap_or(0),
            input_tokens: progress.map(|p| p.input_tokens).unwrap_or(0),
            output_tokens: progress.map(|p| p.output_tokens).unwrap_or(0),
            cache_read_tokens: progress.map(|p| p.cache_read_tokens).unwrap_or(0),
            tool_uses: progress.map(|p| p.tool_use_count).unwrap_or(0),
            duration_ms,
            cost_usd: progress
                .map(|p| p.cost_micro_usd as f64 / 1_000_000.0)
                .unwrap_or(0.0),
            input_cost_usd: progress
                .map(|p| p.input_cost_micro_usd as f64 / 1_000_000.0)
                .unwrap_or(0.0),
            output_cost_usd: progress
                .map(|p| p.output_cost_micro_usd as f64 / 1_000_000.0)
                .unwrap_or(0.0),
        };
        let params = TaskCompletedParams {
            task_id: task_id.to_string(),
            tool_use_id: state.tool_use_id.clone(),
            status,
            killed_by: state.killed_by,
            output_file,
            summary: state.description.clone(),
            usage: Some(usage),
        };
        let _ = tx
            .send(CoreEvent::Protocol(ServerNotification::TaskCompleted(
                params,
            )))
            .await;
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KillTaskError {
    #[error("task not found")]
    NotFound,
    #[error("task is not running")]
    NotRunning,
}

/// What [`TaskManager::claim_workflow_emit`] decided for one delta.
enum WorkflowEmitDecision {
    /// Publish immediately — the coalescing window has elapsed.
    Now,
    /// Publish after this delay; this delta owns the trailing flush.
    After(std::time::Duration),
    /// Nothing to do: a trailing flush is already pending and will read the
    /// array fresh, so it carries this delta too.
    Coalesced,
}

/// Build the `task/progress` payload for one task. Free-standing so the
/// detached trailing flush can reuse it without a `TaskManager` handle.
fn task_progress_params(task_id: &str, state: &TaskStateBase) -> TaskProgressParams {
    TaskProgressParams {
        task_id: task_id.to_string(),
        tool_use_id: state.tool_use_id.clone(),
        description: state.description.clone(),
        usage: TaskUsage {
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            tool_uses: 0,
            duration_ms: current_time_ms().saturating_sub(state.start_time),
            cost_usd: 0.0,
            input_cost_usd: 0.0,
            output_cost_usd: 0.0,
        },
        last_tool_name: None,
        summary: None,
        agent_type: state.progress().and_then(|p| p.agent_type.clone()),
        recent_activities: Vec::new(),
        workflow_progress: workflow_progress(state),
    }
}

/// Publish one task's current progress array. Re-reads the row rather than
/// taking a snapshot argument, so a delayed flush always carries the newest
/// state instead of the state as of when it was scheduled.
async fn emit_workflow_frame(
    rows: &RwLock<HashMap<String, TaskStateBase>>,
    event_tx: Option<&mpsc::Sender<CoreEvent>>,
    task_id: &str,
) {
    let Some(tx) = event_tx else { return };
    let params = {
        let rows = rows.read().await;
        let Some(state) = rows.get(task_id) else {
            return;
        };
        task_progress_params(task_id, state)
    };
    let _ = tx
        .send(CoreEvent::Protocol(ServerNotification::TaskProgress(
            params,
        )))
        .await;
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Re-export of [`TaskType::wire_name`] kept as a free function for
/// callers that already imported it under this path. The canonical
/// definition lives on [`TaskType`] in `coco_types` so the matching
/// `coco_types::task_type_wire` constants stay paired with it.
pub fn task_type_wire_name(task_type: TaskType) -> &'static str {
    task_type.wire_name()
}

fn workflow_progress(state: &TaskStateBase) -> Vec<WorkflowProgressEvent> {
    match &state.extras {
        TaskExtras::LocalWorkflow(extras) => extras.workflow_progress.clone(),
        _ => Vec::new(),
    }
}

/// Map the terminal [`TaskStatus`] onto the SDK-facing
/// [`TaskCompletionStatus`]. Only called from [`TaskManager::emit_task_completed`],
/// which itself only fires after [`TaskManager::transition_terminal`] has set
/// a terminal status. A `Pending` / `Running` value here is a caller bug.
fn task_status_to_completion(status: TaskStatus) -> TaskCompletionStatus {
    match status {
        TaskStatus::Completed => TaskCompletionStatus::Completed,
        TaskStatus::Failed => TaskCompletionStatus::Failed,
        TaskStatus::Killed => TaskCompletionStatus::Stopped,
        TaskStatus::Pending | TaskStatus::Running => {
            unreachable!("emit_task_completed called with non-terminal status {status:?}")
        }
    }
}

#[cfg(test)]
#[path = "running.test.rs"]
mod tests;
