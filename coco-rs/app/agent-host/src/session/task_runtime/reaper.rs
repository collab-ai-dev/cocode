//! Memory-pressure cleanup for idle background shell tasks.
//!
//! This is intentionally conservative: it only cancels top-level
//! background shell tasks that have been idle for a long time, and it
//! stands down while agents or foreground tasks are still running.

use std::sync::Arc;
use std::time::Duration;

use coco_types::{TaskKilledBy, TaskStatus, TaskType};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::TaskRuntime;

const REAPER_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_MS: i64 = 30 * 60 * 1000;
const MIN_AVAILABLE_BYTES: u64 = 512 * 1024 * 1024;
const MIN_AVAILABLE_RATIO: f64 = 0.10;

impl TaskRuntime {
    pub fn start_memory_pressure_shell_reaper(self: &Arc<Self>, shutdown: CancellationToken) {
        if coco_config::env::is_env_truthy(
            coco_config::env::EnvKey::CocoDisableMemoryPressureShellReaper,
        ) {
            debug!(
                target: "coco::task_runtime::reaper",
                "memory-pressure shell reaper disabled by env"
            );
            return;
        }
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REAPER_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = ticker.tick() => {}
                }
                if !memory_pressure() {
                    continue;
                }
                let killed = runtime
                    .reap_idle_background_shells_for_memory_pressure(now_ms(), IDLE_MS)
                    .await;
                if !killed.is_empty() {
                    warn!(
                        target: "coco::task_runtime::reaper",
                        count = killed.len(),
                        tasks = ?killed,
                        "memory pressure: stopped idle background shell task(s)"
                    );
                }
            }
        });
    }

    pub(in crate::session::task_runtime) async fn reap_idle_background_shells_for_memory_pressure(
        &self,
        now_ms: i64,
        idle_ms: i64,
    ) -> Vec<String> {
        let states = self.manager.list().await;
        if states.iter().any(|state| {
            state.status == TaskStatus::Running
                && matches!(
                    state.task_type(),
                    TaskType::BgAgent | TaskType::Teammate | TaskType::RemoteTeammate
                )
        }) {
            return Vec::new();
        }
        if states
            .iter()
            .any(|state| state.status == TaskStatus::Running && !state.is_backgrounded())
        {
            return Vec::new();
        }

        let mut killed = Vec::new();
        for state in states {
            if state.status != TaskStatus::Running || state.task_type() != TaskType::Shell {
                continue;
            }
            let Some(extras) = state.shell_extras() else {
                continue;
            };
            if !extras.is_backgrounded || extras.issuing_agent.is_some() {
                continue;
            }
            let last_activity = last_activity_ms(&state).await.unwrap_or(state.start_time);
            if now_ms.saturating_sub(last_activity) < idle_ms {
                continue;
            }
            match self
                .manager
                .kill_running_by(&state.id, TaskKilledBy::System)
                .await
            {
                Ok(()) => killed.push(state.id),
                Err(
                    coco_tasks::KillTaskError::NotFound | coco_tasks::KillTaskError::NotRunning,
                ) => {}
            }
        }
        killed
    }
}

async fn last_activity_ms(state: &coco_types::TaskStateBase) -> Option<i64> {
    let path = state.output_file.as_ref()?;
    let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn memory_pressure() -> bool {
    let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") else {
        return false;
    };
    let mut total_kb = None;
    let mut available_kb = None;
    for line in meminfo.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total_kb = parse_meminfo_kb(value);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available_kb = parse_meminfo_kb(value);
        }
    }
    let (Some(total_kb), Some(available_kb)) = (total_kb, available_kb) else {
        return false;
    };
    let available_bytes = available_kb.saturating_mul(1024);
    let ratio = available_kb as f64 / total_kb.max(1) as f64;
    available_bytes < MIN_AVAILABLE_BYTES || ratio < MIN_AVAILABLE_RATIO
}

fn parse_meminfo_kb(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
