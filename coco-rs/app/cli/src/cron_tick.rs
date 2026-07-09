//! Cron tick driver — the timer + fire half of the cron scheduler,
//! wired into the interactive session.
//!
//! Every second it reads the schedule store, asks the pure
//! [`coco_cron::CronTickState`] which tasks crossed a fire boundary, and for
//! each fire enqueues the task's prompt onto the session [`CommandQueue`] with
//! [`QueueOrigin::Cron`]. The enqueue wakes the idle agent driver
//! (`tui_runner::run_agent_driver` selects on `command_queue().wait_for_change`),
//! so the scheduled prompt runs as a turn; if a turn is already in flight it
//! drains at the next turn boundary. Recurring tasks are rescheduled (and their
//! `last_fired_at` persisted); one-shot / aged tasks are removed.
//!
//! Deferred: cross-process lease lock, the chokidar file-watcher
//! (the 1s tick re-reads the file every pass, so external edits are picked up
//! within ≤1s), jitter, and the missed-task AskUserQuestion variant
//! (missed one-shots are surfaced as a batched notification — see
//! [`build_missed_notification`]).
//!
//! TUI-only: the headless (`coco -p`) and SDK paths are one-shot / have no
//! queue-drain pump, so a fired prompt would have nobody to run it. Durable
//! tasks created in those modes still persist to disk and fire in a later
//! interactive session.

use std::sync::Arc;
use std::time::Duration;

use coco_cron::CronTickState;
use coco_cron::CronTiming;
use coco_cron::RECURRING_MAX_AGE_MS;
use coco_query::QueuePriority;
use coco_query::QueuedCommand;
use coco_system_reminder::QueueOrigin;
use coco_tool_runtime::CronTask;
use coco_types::SessionId;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::session_runtime::SessionHandle;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn timing(t: &CronTask) -> CronTiming<'_> {
    CronTiming {
        id: &t.id,
        cron: &t.cron,
        created_at_ms: t.created_at,
        last_fired_at_ms: t.last_fired_at,
        recurring: t.is_recurring(),
        permanent: t.permanent.unwrap_or(false),
    }
}

/// TUI-lifetime cron tick task. Dropping the guard stops the detached loop.
pub struct CronTickGuard {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl Drop for CronTickGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// Spawn the cron tick for a fixed session handle.
pub fn spawn(session: SessionHandle) -> CronTickGuard {
    spawn_current_session(Arc::new(tokio::sync::RwLock::new(session)))
}

/// Spawn the TUI cron tick against a swappable current-session owner.
///
/// The task lives for the TUI lifetime and resolves the current session on each
/// tick. After startup resume, `/resume`, `/branch`, or `/clear`, scheduled
/// prompts are enqueued into the replacement runtime's command queue instead of
/// a stale startup runtime.
pub fn spawn_current_session(
    current_session: Arc<tokio::sync::RwLock<SessionHandle>>,
) -> CronTickGuard {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        let mut state = CronTickState::new();
        let mut active_session_id: Option<SessionId> = None;
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let session = current_session.read().await.clone();
                    let session_id = session.session_id().clone();
                    if active_session_id.as_ref() != Some(&session_id) {
                        state = CronTickState::new();
                        process_missed_for_session(&session).await;
                        active_session_id = Some(session_id);
                    }
                    process_tick_for_session(&session, &mut state).await;
                }
                _ = task_cancel.cancelled() => break,
            }
        }
    });

    CronTickGuard { cancel, task }
}

async fn process_missed_for_session(session: &SessionHandle) {
    let runtime = session.runtime().clone();
    let store = runtime.schedule_store();
    let queue = runtime.command_queue().clone();

    // Surface missed one-shot tasks as one batched notification, then remove
    // them so the tick doesn't fire them directly. Recurring tasks that came
    // due while down fire on the next scheduler tick below.
    let initial = store.list_all_cron_tasks().await.unwrap_or_default();
    let now0 = now_ms();
    let missed_ids: Vec<String> = {
        let timings: Vec<CronTiming> = initial.iter().map(timing).collect();
        coco_cron::find_missed(&timings, now0)
    };
    if missed_ids.is_empty() {
        return;
    }

    let missed: Vec<&CronTask> = initial
        .iter()
        .filter(|t| missed_ids.iter().any(|m| m == &t.id))
        .collect();
    queue
        .enqueue(
            QueuedCommand::new(build_missed_notification(&missed), QueuePriority::Later)
                .with_origin(QueueOrigin::Cron),
        )
        .await;
    let refs: Vec<&str> = missed_ids.iter().map(String::as_str).collect();
    let _ = store.remove_cron_tasks(&refs).await;
}

async fn process_tick_for_session(session: &SessionHandle, state: &mut CronTickState) {
    let runtime = session.runtime().clone();
    let store = runtime.schedule_store();
    let queue = runtime.command_queue().clone();
    let project_root = runtime.project_root().clone();
    let current_cwd = Arc::clone(runtime.current_cwd());
    let loop_sentinel_state = runtime.loop_sentinel_state().clone();
    let loop_persistent_preamble_enabled = runtime
        .runtime_config()
        .loop_config
        .persistent_preamble_enabled;

    let tasks = match store.list_all_cron_tasks().await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(target: "coco::cron", error = %e, "schedule read failed");
            return;
        }
    };
    let now = now_ms();
    let fires = {
        let timings: Vec<CronTiming> = tasks.iter().map(timing).collect();
        state.tick(&timings, now, RECURRING_MAX_AGE_MS)
    };
    for fire in fires {
        if let Some(task) = tasks.iter().find(|t| t.id == fire.id) {
            tracing::info!(
                target: "coco::cron",
                id = %fire.id, recurring = fire.recurring, aged = fire.aged,
                "scheduled task fired"
            );
            let cwd = current_cwd.read().await.clone();
            let prompt = {
                let mut state = loop_sentinel_state.lock().await;
                expand_scheduled_prompt(
                    &task.prompt,
                    &project_root,
                    &cwd,
                    &mut state,
                    loop_persistent_preamble_enabled,
                )
            };
            queue
                .enqueue(
                    QueuedCommand::new(prompt, QueuePriority::Later).with_origin(QueueOrigin::Cron),
                )
                .await;
        }
        if fire.recurring && !fire.aged {
            let _ = store.mark_cron_tasks_fired(&[&fire.id], now).await;
        } else {
            let _ = store.remove_cron_tasks(&[&fire.id]).await;
        }
    }
}

fn expand_scheduled_prompt(
    prompt: &str,
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    state: &mut coco_skills::bundled::loop_skill::LoopSentinelState,
    persistent_preamble_enabled: bool,
) -> String {
    coco_skills::bundled::loop_skill::expand_sentinel_prompt_with_state(
        prompt,
        project_root,
        cwd,
        state,
        persistent_preamble_enabled,
    )
    .unwrap_or_else(|| prompt.to_string())
}

/// Batched "missed while not running" notification. Guidance precedes the task
/// list; each prompt is wrapped in a backtick fence one longer than any run
/// inside it so a prompt containing ``` can't break out (prompt-injection guard).
pub fn build_missed_notification(missed: &[&CronTask]) -> String {
    let plural = missed.len() > 1;
    let (were, they, them, these) = if plural {
        ("s were", "They have", "these prompts", "each one")
    } else {
        (" was", "It has", "this prompt", "it")
    };
    let schedule_path = format!(
        "{}/scheduled_tasks.json",
        coco_utils_common::COCO_CONFIG_DIR_NAME
    );
    let header = format!(
        "The following one-shot scheduled task{were} missed while Coco was not running. \
         {they} already been removed from {schedule_path}.\n\n\
         Do NOT execute {these} yet. First use the AskUserQuestion tool to ask whether to run \
         {them} now. Only execute if the user confirms."
    );
    let blocks: Vec<String> = missed
        .iter()
        .map(|t| {
            let longest = t
                .prompt
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let fence = "`".repeat(longest.max(2) + 1);
            let meta = coco_cron::cron_to_human(&t.cron);
            format!("[{meta}]\n{fence}\n{}\n{fence}", t.prompt)
        })
        .collect();
    format!("{header}\n\n{}", blocks.join("\n\n"))
}

#[cfg(test)]
#[path = "cron_tick.test.rs"]
mod tests;
