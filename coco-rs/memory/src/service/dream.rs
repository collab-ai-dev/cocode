//! Auto-dream consolidation service.
//!
//! Three-gate scheduling:
//!
//! 1. **Time** — at least `dream_min_hours` since last consolidation.
//! 2. **Sessions** — at least `dream_min_sessions` distinct sessions
//!    have produced transcripts since the last consolidation.
//! 3. **Lock** — exactly one consolidation in flight, enforced by
//!    [`crate::lock`] PID + mtime CAS.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use arc_swap::ArcSwap;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnConstraints;
use coco_tool_runtime::AgentSpawnRequest;
use coco_types::ActiveShellTool;
use coco_types::ModelRole;

use crate::config::MemoryConfig;
use crate::lock;
use crate::lock::LockOutcome;
use crate::prompt::build_dream_prompt;
use crate::telemetry::MemoryEvent;
use crate::telemetry::MemoryTelemetryEmitter;
use crate::telemetry::NoopEmitter;

/// Scan throttle — bail if we already attempted a consolidation
/// within this window (`SESSION_SCAN_INTERVAL_MS = 600_000`).
pub const SCAN_THROTTLE: Duration = Duration::from_secs(10 * 60);

/// One per-call outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamOutcome {
    Skipped(SkipReason),
    Completed { duration_ms: i64 },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    KairosMode,
    TimeGate {
        hours_since: i64,
    },
    SessionGate {
        sessions_seen: i32,
    },
    LockHeld,
    ScanThrottled,
    /// Another dream from THIS process is already in-flight. With
    /// `lock::try_acquire` reclaiming same-process locks (so manual
    /// `/dream` works after a successful auto-dream), the lock file
    /// no longer provides within-process exclusion; this atomic does.
    InProgress,
}

/// RAII guard for the within-process `consolidating` flag. Sync Drop
/// clears the atomic so a cancelled `consolidate_with_gates` future
/// doesn't leak the flag and wedge subsequent calls.
struct ConsolidatingGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for ConsolidatingGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// Auto-dream service.
pub struct DreamService {
    session_id: ArcSwap<String>,
    memory_dir: PathBuf,
    config: MemoryConfig,
    agent: crate::service::extract::AgentSlot,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
    active_shell_tool: ActiveShellTool,
    /// User-visible notice channel — engine drains the inbox once per
    /// turn and injects a `SystemMemorySavedMessage` with `verb:
    /// "Improved"`.
    notices: crate::notice::NoticeInbox,
    /// Scan-throttle stamp. `std::sync::Mutex` (not tokio) because the
    /// critical section is two cheap operations on `Option<Instant>`
    /// — no `.await` inside, no need for the async runtime hop.
    last_scan_at: std::sync::Mutex<Option<Instant>>,
    /// Within-process consolidation in-flight flag. Required because
    /// `lock::try_acquire` reclaims same-process locks (so a manual
    /// `/dream` works after a successful auto-dream); without this
    /// atomic, two concurrent `consolidate_with_gates` calls from the
    /// same process (e.g. auto-dream mid-flight + user-typed `/dream`)
    /// would both reach `try_acquire`, both reclaim the lock, and both
    /// run consolidations in parallel — corrupting MEMORY.md.
    consolidating: Arc<AtomicBool>,
}

impl std::fmt::Debug for DreamService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DreamService")
            .field("memory_dir", &self.memory_dir)
            .field("dream_enabled", &self.config.dream_enabled)
            .finish()
    }
}

impl DreamService {
    pub fn new(memory_dir: PathBuf, config: MemoryConfig, agent: AgentHandleRef) -> Self {
        Self::with_shared_agent(
            memory_dir,
            config,
            Arc::new(std::sync::RwLock::new(agent)),
            Arc::new(NoopEmitter),
        )
    }

    /// Shared-cell constructor — used by [`crate::MemoryRuntimeBuilder`]
    /// so all three services see the same swappable handle.
    pub fn with_shared_agent(
        memory_dir: PathBuf,
        config: MemoryConfig,
        agent: crate::service::extract::AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
    ) -> Self {
        Self::with_shared_agent_and_notices(
            memory_dir,
            config,
            agent,
            telemetry,
            crate::notice::NoticeInbox::default(),
            ActiveShellTool::Disabled,
            "test-session".to_string(),
        )
    }

    /// Full constructor — `MemoryRuntimeBuilder` uses this so the
    /// inbox is shared with the runtime's drain endpoint.
    pub fn with_shared_agent_and_notices(
        memory_dir: PathBuf,
        config: MemoryConfig,
        agent: crate::service::extract::AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
        notices: crate::notice::NoticeInbox,
        active_shell_tool: ActiveShellTool,
        session_id: String,
    ) -> Self {
        Self {
            session_id: ArcSwap::from_pointee(session_id),
            memory_dir,
            config,
            agent,
            telemetry,
            active_shell_tool,
            notices,
            last_scan_at: std::sync::Mutex::new(None),
            consolidating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_session_id(&self, new_id: String) {
        self.session_id.store(Arc::new(new_id));
    }

    /// Try to atomically claim the within-process consolidation slot.
    /// Returns a Drop guard on success; `None` if another caller is
    /// already running. The guard's `Drop` synchronously clears the
    /// flag so a cancelled future can't leak.
    fn try_claim_consolidating(&self) -> Option<ConsolidatingGuard> {
        self.consolidating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ConsolidatingGuard {
                flag: self.consolidating.clone(),
            })
    }

    /// `transcript_dir` is the project's session-transcript root used by
    /// the agent for narrow grep. `enumerate_sessions` lazily produces
    /// the session-ID slice — invoked **only** after the time + scan
    /// gates pass so callers don't pay the directory walk on every
    /// turn (`listSessionsTouchedSince` only runs after the time gate).
    /// `now_ms` is the current wall clock — accept it as a parameter so
    /// tests stay deterministic.
    pub async fn maybe_consolidate<F>(
        &self,
        transcript_dir: &std::path::Path,
        enumerate_sessions: F,
        now_ms: i64,
    ) -> DreamOutcome
    where
        F: FnOnce() -> Vec<String> + Send,
    {
        self.consolidate_with_gates(transcript_dir, enumerate_sessions, now_ms, false)
            .await
    }

    /// Force a consolidation regardless of the time / session / scan
    /// throttle gates — bound to the `/dream` slash command. Still
    /// honors the `dream_enabled` and `kairos_mode` settings (manual
    /// `/dream` runs as the disk-skill in the main loop, but auto-dream
    /// is never invoked when these are off). The PID + mtime
    /// CAS lock is still acquired so a manual run cannot race with an
    /// auto-dream in flight. The `enumerate_sessions` closure is
    /// invoked unconditionally under force so the prompt's
    /// session-hint block reflects whatever the caller knows about.
    pub async fn force<F>(
        &self,
        transcript_dir: &std::path::Path,
        enumerate_sessions: F,
        now_ms: i64,
    ) -> DreamOutcome
    where
        F: FnOnce() -> Vec<String> + Send,
    {
        self.consolidate_with_gates(transcript_dir, enumerate_sessions, now_ms, true)
            .await
    }

    async fn consolidate_with_gates<F>(
        &self,
        transcript_dir: &std::path::Path,
        enumerate_sessions: F,
        now_ms: i64,
        force: bool,
    ) -> DreamOutcome
    where
        F: FnOnce() -> Vec<String> + Send,
    {
        if !self.config.dream_enabled {
            return DreamOutcome::Skipped(SkipReason::Disabled);
        }
        if self.config.kairos_mode {
            return DreamOutcome::Skipped(SkipReason::KairosMode);
        }

        // Within-process exclusion. Claim BEFORE the time/scan/session
        // gates so a concurrent auto-dream + manual `/dream` from the
        // same process serialize correctly. The lock file is now
        // same-process-reclaimable (see `lock::try_acquire`), so we
        // can't rely on it for within-process serialization.
        //
        // The RAII guard's Drop clears the flag synchronously, so a
        // cancelled `consolidate_with_gates` future doesn't wedge
        // subsequent calls.
        let _consolidating_guard = match self.try_claim_consolidating() {
            Some(g) => g,
            None => return DreamOutcome::Skipped(SkipReason::InProgress),
        };

        // Time gate first — `lastConsolidatedAt` stat happens before any
        // session scan. Eager `lock::last_consolidated_at` is one stat;
        // cheap regardless of the scan throttle.
        let prior_last_ms = lock::last_consolidated_at(&self.memory_dir);
        let hours_since_initial = prior_last_ms
            .map(|m| (now_ms.saturating_sub(m)) / (60 * 60 * 1000))
            .unwrap_or(i64::MAX);
        if !force && hours_since_initial < self.config.dream_min_hours as i64 {
            return DreamOutcome::Skipped(SkipReason::TimeGate {
                hours_since: hours_since_initial,
            });
        }

        // Snapshot the prior scan-throttle stamp so we can roll back
        // on Failed / cancellation. `lastSessionScanAt` only advances
        // on a real-fired consolidation; if we update before the fork
        // runs and the fork fails, retries within 10 min would be
        // needlessly throttled.
        let prior_scan_at: Option<Instant> = if !force {
            // Scan throttle — `SESSION_SCAN_INTERVAL_MS = 600_000`.
            let mut last = self
                .last_scan_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(t) = *last
                && t.elapsed() < SCAN_THROTTLE
            {
                return DreamOutcome::Skipped(SkipReason::ScanThrottled);
            }
            let prev = *last;
            *last = Some(Instant::now());
            prev
        } else {
            None
        };

        // Session enumeration — lazy, invoked only after the time +
        // scan gates pass so callers don't pay the directory walk on
        // every turn.
        let sessions_since_last = enumerate_sessions();

        if !force && (sessions_since_last.len() as i32) < self.config.dream_min_sessions {
            let sessions_seen = sessions_since_last.len() as i32;
            // Roll back the scan-throttle stamp — no consolidation
            // actually fired, so the next gate-pass should be allowed
            // to scan again without waiting 10 min. Gated on `!force`
            // because under force we never mutated `last_scan_at`;
            // calling restore would clobber a real auto-set value.
            self.restore_scan_at_if_unforced(force, prior_scan_at);
            return DreamOutcome::Skipped(SkipReason::SessionGate { sessions_seen });
        }

        // Lock — kept under both paths so manual /dream and auto-dream
        // never race over MEMORY.md edits. The `LockGuard` RAII type
        // ensures the lock file's mtime is rolled back on cancellation
        // for async-runtime cancellation.
        let lock_guard = match lock::try_acquire(&self.memory_dir) {
            LockOutcome::Acquired(g) => g,
            LockOutcome::Held => {
                self.restore_scan_at_if_unforced(force, prior_scan_at);
                return DreamOutcome::Skipped(SkipReason::LockHeld);
            }
            LockOutcome::Error(e) => {
                self.restore_scan_at_if_unforced(force, prior_scan_at);
                return DreamOutcome::Failed { reason: e };
            }
        };
        let prior_mtime_ms = lock_guard.prior_mtime_ms();

        let sessions_seen = sessions_since_last.len() as i32;
        let hours_since_last = if hours_since_initial == i64::MAX {
            0
        } else {
            hours_since_initial
        };
        self.telemetry.emit(MemoryEvent::AutoDreamFired {
            hours_since_last,
            sessions_since_last: sessions_seen,
        });

        let start = Instant::now();
        let prompt = build_dream_prompt(&self.memory_dir, transcript_dir, &sessions_since_last);
        // Synthetic AgentDefinition pinning `ModelRole::Memory`. See
        // `extract.rs` for the design rationale. Single-source-of-truth:
        // model routing flows through `AgentDefinition.model_role`
        // (the catalog source of truth); memory forks construct an
        // in-process synthetic def at spawn time.
        let memory_def = std::sync::Arc::new(coco_types::AgentDefinition {
            agent_type: coco_types::AgentTypeId::Custom("memory-internal".into()),
            name: "memory-internal".into(),
            model_role: Some(ModelRole::Memory),
            ..Default::default()
        });
        let request = AgentSpawnRequest {
            prompt,
            description: Some("auto-dream consolidation".into()),
            session_id: (**self.session_id.load()).clone(),
            subagent_type: Some("general-purpose".into()),
            definition: Some(memory_def),
            constraints: Some(AgentSpawnConstraints {
                // No cap — the agent stops naturally when it has nothing
                // left to merge. Capping at 20 silently truncated long
                // consolidations.
                max_turns: None,
                allowed_write_roots: vec![self.memory_dir.clone()],
            }),
            // Keep the background subagent's tool-uses out of the user's
            // main JSONL transcript.
            skip_transcript: true,
            // `canUseTool: createAutoMemCanUseTool(memoryRoot)`.
            can_use_tool: Some(crate::can_use_tool::create_auto_mem_handle_with_telemetry(
                self.memory_dir.clone(),
                self.telemetry.clone(),
            )),
            require_can_use_tool: false,
            fork_label: Some(coco_types::ForkLabel::AutoDream),
            active_shell_tool: self.active_shell_tool,
            ..Default::default()
        };

        tracing::info!(
            target: "coco_memory::dream",
            sessions_seen,
            hours_since = hours_since_last,
            "spawning auto-dream consolidation subagent"
        );

        let agent = self
            .agent
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match agent.spawn_agent(request).await {
            Ok(resp) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                tracing::info!(
                    target: "coco_memory::dream",
                    duration_ms,
                    files_changed = resp.total_tool_use_count,
                    sessions_reviewed = sessions_seen,
                    cache_read = resp.cache_read_tokens,
                    cache_create = resp.cache_creation_tokens,
                    "auto-dream consolidation complete"
                );
                let entrypoint = crate::store::ENTRYPOINT_NAME;
                let topic_paths: Vec<String> = resp
                    .paths_written
                    .iter()
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n != entrypoint)
                    })
                    .map(|p| p.display().to_string())
                    .collect();
                self.telemetry.emit(MemoryEvent::AutoDreamCompleted {
                    sessions_reviewed: sessions_seen,
                    files_changed: resp.total_tool_use_count as i32,
                    cache_read_tokens: resp.cache_read_tokens,
                    cache_creation_tokens: resp.cache_creation_tokens,
                    output_tokens: resp.output_tokens,
                    duration_ms,
                });
                if !topic_paths.is_empty() {
                    self.notices.push(crate::notice::MemoryUserNotice {
                        written_paths: topic_paths,
                        verb: crate::notice::NoticeVerb::Improved,
                    });
                }
                if force {
                    // Manual /dream: rollback the mtime so the auto
                    // 24h gate continues counting from the last *real*
                    // periodic consolidation. Also emit a Manual event
                    // so dashboards can split auto vs manual cadence.
                    self.telemetry.emit(MemoryEvent::AutoDreamManual);
                    lock_guard.rollback_now();
                } else {
                    // Non-force success: keep the fresh mtime (it IS
                    // the lastConsolidatedAt stamp the next 24h gate
                    // reads). `commit` so Drop doesn't roll back.
                    lock_guard.commit();
                }
                DreamOutcome::Completed { duration_ms }
            }
            Err(e) => {
                tracing::warn!(
                    target: "coco_memory::dream",
                    error = %e,
                    "auto-dream subagent failed; rolling back lock + scan throttle"
                );
                // Drop on the guard will rollback the lock mtime
                // automatically (rollback_on_drop is true by
                // default), restoring the prior cadence reference.
                // Roll back the scan-throttle stamp only on the
                // auto path — under force we never mutated it.
                drop(lock_guard);
                self.restore_scan_at_if_unforced(force, prior_scan_at);
                self.telemetry.emit(MemoryEvent::AutoDreamFailed);
                let _ = prior_mtime_ms; // kept for tracing breadcrumb
                DreamOutcome::Failed { reason: e }
            }
        }
    }

    /// Restore `last_scan_at` to its pre-attempt value, but only when
    /// the call wasn't forced. Under `force=true` we never mutate
    /// `last_scan_at` (the force path bypasses the scan throttle
    /// entirely), so writing back `prior_scan_at = None` would clobber
    /// any value the auto path had set — corrupting the throttle so
    /// every subsequent auto turn scans freely.
    ///
    /// Used on SessionGate / LockHeld / Failed paths in the auto
    /// branch so a real retry within 10 min isn't blocked by a
    /// phantom scan stamp.
    fn restore_scan_at_if_unforced(&self, force: bool, prior: Option<Instant>) {
        if force {
            return;
        }
        let mut last = self
            .last_scan_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *last = prior;
    }

    /// Wall-clock helper so callers don't have to import `SystemTime`.
    pub fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "dream.test.rs"]
mod tests;
