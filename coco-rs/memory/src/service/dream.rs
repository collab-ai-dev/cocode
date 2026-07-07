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

use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnConstraints;
use coco_tool_runtime::AgentSpawnExecution;
use coco_tool_runtime::AgentSpawnInheritance;
use coco_tool_runtime::AgentSpawnInput;
use coco_tool_runtime::AgentSpawnPermissions;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnRouting;
use coco_tool_runtime::AgentSpawnTelemetry;
use coco_types::ActiveShellTool;
use coco_types::ModelRole;
use coco_types::SessionId;
use coco_types::ToolOverrides;

use coco_background_review::LockOutcome;

use crate::config::MemoryConfig;
use crate::lock::consolidate_lock;
use crate::prompt::FileMutationPromptTools;
use crate::prompt::build_dream_prompt;
use crate::service::MemoryForkToolConfig;
use crate::service::SessionIdSlot;
use crate::telemetry::AutoDreamFailurePhase;
use crate::telemetry::AutoDreamSkipReason;
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

#[derive(Clone)]
pub(crate) struct DreamNoticeChannels {
    notices: crate::notice::NoticeInbox,
    memory_updates: crate::notice::MemoryUpdateInbox,
}

impl DreamNoticeChannels {
    pub(crate) fn new(
        notices: crate::notice::NoticeInbox,
        memory_updates: crate::notice::MemoryUpdateInbox,
    ) -> Self {
        Self {
            notices,
            memory_updates,
        }
    }
}

fn error_class_from_message(error: &str) -> String {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        return "Error".to_string();
    }

    let Some((prefix, _)) = trimmed.split_once(':') else {
        return "Error".to_string();
    };
    let class = prefix.trim();
    if class.is_empty()
        || !class
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        "Error".to_string()
    } else {
        class.to_string()
    }
}

async fn count_daily_logs(memory_dir: &std::path::Path) -> i32 {
    let logs_dir = memory_dir.join("logs");
    let mut stack = vec![logs_dir.clone()];
    let mut count = 0_i32;

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir == logs_dir => return 0,
            Err(e) => {
                tracing::debug!(
                    target: "coco_memory::dream",
                    path = %dir.display(),
                    error = %e,
                    "countDailyLogs failed"
                );
                return 0;
            }
        };

        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(
                        target: "coco_memory::dream",
                        path = %dir.display(),
                        error = %e,
                        "countDailyLogs failed"
                    );
                    return 0;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(e) => {
                    tracing::debug!(
                        target: "coco_memory::dream",
                        path = %path.display(),
                        error = %e,
                        "countDailyLogs failed"
                    );
                    return 0;
                }
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                count += 1;
            }
        }
    }

    count
}

/// Auto-dream service.
pub struct DreamService {
    session_id: SessionIdSlot,
    memory_dir: PathBuf,
    config: MemoryConfig,
    agent: crate::service::extract::AgentSlot,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
    active_shell_tool: ActiveShellTool,
    tool_overrides: Arc<ToolOverrides>,
    /// User-visible notice channel — engine drains the inbox once per
    /// turn and injects a `SystemMemorySavedMessage` with `verb:
    /// "Improved"`.
    notices: crate::notice::NoticeInbox,
    /// Model-visible update channel — engine drains this once per turn
    /// and tells the main model background consolidation changed files
    /// it may have cached in context.
    memory_updates: crate::notice::MemoryUpdateInbox,
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
            MemoryForkToolConfig::disabled(),
            match SessionId::try_new("test-session") {
                Ok(id) => id,
                Err(_) => unreachable!("test session id must be valid"),
            },
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
        tool_config: MemoryForkToolConfig,
        session_id: SessionId,
    ) -> Self {
        Self::with_shared_agent_and_notice_channels(
            memory_dir,
            config,
            agent,
            telemetry,
            DreamNoticeChannels::new(notices, crate::notice::MemoryUpdateInbox::default()),
            tool_config,
            session_id,
        )
    }

    pub(crate) fn with_shared_agent_and_notice_channels(
        memory_dir: PathBuf,
        config: MemoryConfig,
        agent: crate::service::extract::AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
        channels: DreamNoticeChannels,
        tool_config: MemoryForkToolConfig,
        session_id: SessionId,
    ) -> Self {
        let session_id = Arc::new(arc_swap::ArcSwap::from_pointee(session_id));
        Self::with_shared_agent_channels_and_session_id_slot(
            memory_dir,
            config,
            agent,
            telemetry,
            channels,
            tool_config,
            session_id,
        )
    }

    pub(crate) fn with_shared_agent_channels_and_session_id_slot(
        memory_dir: PathBuf,
        config: MemoryConfig,
        agent: crate::service::extract::AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
        channels: DreamNoticeChannels,
        tool_config: MemoryForkToolConfig,
        session_id: SessionIdSlot,
    ) -> Self {
        Self {
            session_id,
            memory_dir,
            config,
            agent,
            telemetry,
            active_shell_tool: tool_config.active_shell_tool,
            tool_overrides: tool_config.tool_overrides,
            notices: channels.notices,
            memory_updates: channels.memory_updates,
            last_scan_at: std::sync::Mutex::new(None),
            consolidating: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_session_id(&self, new_id: SessionId) {
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
        // No team-server "has content" gate here: coco-rs intentionally
        // supports personal-only background dream without team sync.

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
        // session scan. Eager `last_consolidated_at` is one stat;
        // cheap regardless of the scan throttle.
        let dream_lock = consolidate_lock(&self.memory_dir);
        let prior_last_ms = dream_lock.last_consolidated_at();
        let hours_since_initial = prior_last_ms
            .map(|m| (now_ms.saturating_sub(m)) / (60 * 60 * 1000))
            .unwrap_or(i64::MAX);
        if !force && hours_since_initial < self.config.dream_min_hours as i64 {
            return DreamOutcome::Skipped(SkipReason::TimeGate {
                hours_since: hours_since_initial,
            });
        }

        // Scan throttle — `SESSION_SCAN_INTERVAL_MS = 600_000`.
        // Upstream treats this as a session-scan attempt throttle:
        // once the time gate passes and we pay the transcript scan,
        // the stamp stays even if the session gate, lock, or fork
        // later fails. That prevents every following turn from
        // re-walking transcripts while the time gate remains open.
        if !force {
            let mut last = self
                .last_scan_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(t) = *last
                && t.elapsed() < SCAN_THROTTLE
            {
                return DreamOutcome::Skipped(SkipReason::ScanThrottled);
            }
            *last = Some(Instant::now());
        }

        // Session enumeration — lazy, invoked only after the time +
        // scan gates pass so callers don't pay the directory walk on
        // every turn.
        let sessions_since_last = enumerate_sessions();

        if !force && (sessions_since_last.len() as i32) < self.config.dream_min_sessions {
            let sessions_seen = sessions_since_last.len() as i32;
            self.telemetry.emit(MemoryEvent::AutoDreamSkipped {
                reason: AutoDreamSkipReason::Sessions,
                session_count: Some(sessions_seen),
                min_required: Some(self.config.dream_min_sessions),
            });
            return DreamOutcome::Skipped(SkipReason::SessionGate { sessions_seen });
        }

        // Lock — kept under both paths so manual /dream and auto-dream
        // never race over MEMORY.md edits. The `LockGuard` RAII type
        // ensures the lock file's mtime is rolled back on cancellation
        // for async-runtime cancellation.
        let lock_guard = match dream_lock.try_acquire() {
            LockOutcome::Acquired(g) => g,
            LockOutcome::Held => {
                self.telemetry.emit(MemoryEvent::AutoDreamSkipped {
                    reason: AutoDreamSkipReason::Lock,
                    session_count: None,
                    min_required: None,
                });
                return DreamOutcome::Skipped(SkipReason::LockHeld);
            }
            LockOutcome::Error(e) => {
                return DreamOutcome::Failed { reason: e };
            }
        };

        let sessions_seen = sessions_since_last.len() as i32;
        let hours_since_last = if hours_since_initial == i64::MAX {
            0
        } else {
            hours_since_initial
        };
        let team_memory_enabled = self.config.is_team_recall_enabled();
        self.telemetry.emit(MemoryEvent::AutoDreamFired {
            hours_since_last,
            sessions_since_last: sessions_seen,
            team_memory_enabled,
        });

        let start = Instant::now();
        let daily_logs_found = count_daily_logs(&self.memory_dir).await;
        let prompt = build_dream_prompt(
            &self.memory_dir,
            transcript_dir,
            &sessions_since_last,
            team_memory_enabled,
            FileMutationPromptTools {
                write_tool: self.tool_overrides.write_tool(),
                edit_tool: self.tool_overrides.edit_tool(),
            },
        );
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
            input: AgentSpawnInput {
                prompt,
                description: Some("auto-dream consolidation".into()),
                subagent_type: Some("general-purpose".into()),
                definition: Some(memory_def),
                ..Default::default()
            },
            execution: AgentSpawnExecution {
                // Keep the background subagent's tool-uses out of the user's
                // main JSONL transcript.
                skip_transcript: true,
                ..Default::default()
            },
            permissions: AgentSpawnPermissions {
                constraints: Some(AgentSpawnConstraints {
                    // No cap — the agent stops naturally when it has nothing
                    // left to merge. Capping at 20 silently truncated long
                    // consolidations.
                    max_turns: None,
                    allowed_write_roots: vec![self.memory_dir.clone()],
                }),
                // Dream variant also permits `rm` of `.md` files under
                // the memory directory so stale topics can be pruned.
                can_use_tool: Some(
                    crate::can_use_tool::create_auto_dream_handle_with_telemetry(
                        self.memory_dir.clone(),
                        self.telemetry.clone(),
                    ),
                ),
                ..Default::default()
            },
            inheritance: AgentSpawnInheritance {
                active_shell_tool: self.active_shell_tool,
                ..Default::default()
            },
            routing: AgentSpawnRouting {
                session_id: Some((**self.session_id.load()).clone()),
                ..Default::default()
            },
            telemetry: AgentSpawnTelemetry {
                fork_label: Some(coco_types::ForkLabel::AutoDream),
                ..Default::default()
            },
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
                    files_touched_count = resp.paths_written.len(),
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
                let files_touched_count = resp.paths_written.len() as i32;
                self.telemetry.emit(MemoryEvent::AutoDreamCompleted {
                    sessions_reviewed: sessions_seen,
                    files_touched_count,
                    team_memory_enabled,
                    daily_logs_found,
                    cache_read_tokens: resp.cache_read_tokens,
                    cache_creation_tokens: resp.cache_creation_tokens,
                    output_tokens: resp.output_tokens,
                    duration_ms,
                });
                if !topic_paths.is_empty() {
                    let count = topic_paths.len();
                    let noun = if count == 1 {
                        "memory file"
                    } else {
                        "memory files"
                    };
                    self.notices.push(crate::notice::MemoryUserNotice {
                        written_paths: topic_paths.clone(),
                        verb: crate::notice::NoticeVerb::Improved,
                    });
                    self.memory_updates.push(crate::notice::MemoryUpdateNotice {
                        source: crate::notice::MemoryUpdateSource::Dream,
                        summary: format!("consolidated {count} {noun}"),
                        paths: topic_paths,
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
                    "auto-dream subagent failed; rolling back lock"
                );
                // Drop on the guard will rollback the lock mtime
                // automatically (rollback_on_drop is true by
                // default), restoring the prior cadence reference.
                drop(lock_guard);
                self.telemetry.emit(MemoryEvent::AutoDreamFailed {
                    phase: AutoDreamFailurePhase::Fork,
                    error_class: error_class_from_message(&e),
                });
                DreamOutcome::Failed { reason: e }
            }
        }
    }

    /// Wall-clock helper so callers don't have to import `SystemTime`.
    /// Delegates to the shared [`coco_utils_common::now_epoch_ms`] and
    /// applies the infallible-fallback at the edge.
    pub fn now_ms() -> i64 {
        coco_utils_common::now_epoch_ms().unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "dream.test.rs"]
mod tests;
