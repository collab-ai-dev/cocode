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
use tokio::task::JoinHandle;

use crate::session_runtime::SessionRuntime;

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

/// Spawn the per-session cron tick. Self-terminates on the session shutdown
/// signal. Hold the returned handle for the session's lifetime (drop = let it
/// run until cancel, same pattern as the other watchers).
pub fn spawn(runtime: Arc<SessionRuntime>) -> JoinHandle<()> {
    let cancel = runtime.shutdown_signal();
    let store = runtime.schedule_store();
    let queue = runtime.command_queue().clone();

    tokio::spawn(async move {
        let mut state = CronTickState::new();

        // Startup: surface missed one-shot tasks as one batched notification,
        // then remove them so the tick doesn't fire them directly.
        // Recurring tasks that came due while down fire on the first tick below.
        let initial = store.list_all_cron_tasks().await.unwrap_or_default();
        let now0 = now_ms();
        let missed_ids: Vec<String> = {
            let timings: Vec<CronTiming> = initial.iter().map(timing).collect();
            coco_cron::find_missed(&timings, now0)
        };
        if !missed_ids.is_empty() {
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

        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let tasks = match store.list_all_cron_tasks().await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::debug!(target: "coco::cron", error = %e, "schedule read failed");
                            continue;
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
                            queue
                                .enqueue(
                                    QueuedCommand::new(task.prompt.clone(), QueuePriority::Later)
                                        .with_origin(QueueOrigin::Cron),
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
                _ = cancel.cancelled() => break,
            }
        }
    })
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
    let header = format!(
        "The following one-shot scheduled task{were} missed while Claude was not running. \
         {they} already been removed from .coco/scheduled_tasks.json.\n\n\
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
