//! Session-memory service: 9-section markdown insights per session.
//!
//! Distinct from compact's `SessionMemoryConfig` — that one is a
//! compact-time summary; this is a structured 9-section document the
//! model edits incrementally during the conversation, capped at
//! per-section + total-section token budgets.
//!
//! Trigger gates:
//! - Init: total context tokens ≥ `session_memory_init_tokens` (once).
//! - Update: (token growth ≥ update threshold) AND ((tool calls ≥ 3) OR
//!   (no tool calls last turn — natural break)).
//!
//! Storage: one file per session at
//! `<memory_base>/projects/<slug>/<session_id>/session-memory/summary.md`,
//! mode 0o600, directory mode 0o700. The per-project slug pins the
//! file to the right project so unrelated sessions don't share state.
//!
//! ## Cancellation safety
//!
//! See `extract.rs` for the broader rationale. Session-memory holds
//! `in_progress` in an `Arc<AtomicBool>` with a RAII `InProgressGuard`
//! so a dropped `maybe_extract` future can't leak the flag and wedge
//! the service. The watch channel (`extract_done_tx`) replaces the
//! previous `Notify`, eliminating the notify-after-check race —
//! `Receiver::changed()` is edge-triggered AND remembers the latest
//! value, so a transition that fires between the state read and the
//! `.await` is still observed on the next iteration.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use arc_swap::ArcSwap;
use coco_paths::ProjectPaths;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnConstraints;
use coco_tool_runtime::AgentSpawnRequest;
use coco_types::ActiveShellTool;
use coco_types::ModelRole;
use tokio::sync::Mutex;
use tokio::sync::watch;
use uuid::Uuid;

use crate::compact_truncate::truncate_session_memory_for_compact;
use crate::config::MemoryConfig;
use crate::prompt::build_session_memory_template;
use crate::prompt::build_session_memory_update_prompt;
use crate::telemetry::MemoryEvent;
use crate::telemetry::MemoryTelemetryEmitter;
use crate::telemetry::NoopEmitter;

/// `wait_for_extraction` default timeout — 15s.
pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Stale extraction threshold — 60s. Past this we don't wait, the
/// extraction is presumed crashed.
pub const STALE_THRESHOLD: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct SessionState {
    initialized: bool,
    last_extraction_tokens: i64,
    last_extraction_tool_calls: i32,
    /// Last message UUID folded into a successful extraction. Cumulative
    /// tool-call counts are computed since this cursor — mirrors
    /// `lastMemoryMessageUuid`. Kept as `String` because the engine threads
    /// it through callsites that already operate on the JSON-string form;
    /// converting at the boundary is cheaper than UUID round-tripping every
    /// turn.
    last_extraction_message_uuid: Option<String>,
    /// Last message UUID up to which the SM file is **safely** caught
    /// up — only advances when the previous assistant turn had no
    /// tool calls (`updateLastSummarizedMessageIdIfSafe`). Used by
    /// compact/summary readers that need to know "where SM has covered to"
    /// without risking orphaned tool_results in a downstream summary.
    last_summarized_message_uuid: Option<String>,
    /// Wall-clock at which the current in-flight extraction started.
    /// Only `Some` when `in_progress.load()` is `true`; the
    /// [`InProgressGuard`] clears it on drop in lockstep with the flag.
    extraction_started_at: Option<Instant>,
}

/// RAII guard for the session-memory `in_progress` flag. Constructed
/// only after a CAS `false → true`; `Drop` synchronously resets the
/// flag, clears `extraction_started_at`, and pulses the watch channel.
struct InProgressGuard {
    flag: Arc<AtomicBool>,
    state: Arc<Mutex<SessionState>>,
    notifier: Arc<watch::Sender<bool>>,
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        // Order matters: clear the flag (sync atomic) first so any
        // observer that reads in_progress after the watch wake sees
        // it cleared. Then clear `extraction_started_at` from
        // SessionState — best-effort: a poisoned mutex (panic
        // mid-write) is benign here because the flag is the
        // primary signal.
        self.flag.store(false, Ordering::Release);
        if let Ok(mut state) = self.state.try_lock() {
            state.extraction_started_at = None;
        } else {
            // Mutex is contended — spawn a quick clean-up task.
            // Drop has no .await so we use a sync `try_lock`; if
            // contention is high the `extraction_started_at` field
            // will be reset by the next caller observing in_progress
            // = false via `wait_for_extraction`'s stale check.
        }
        let _ = self.notifier.send_replace(false);
    }
}

/// Per-session memory service.
pub struct SessionMemoryService {
    /// Current session id. Read on every `file_path()` call; mutated
    /// only on `/clear` (`set_session_id`). `ArcSwap` is the right
    /// primitive for this "rare write, frequent sync read" shape:
    /// reads are a single relaxed atomic load, no lock acquisition,
    /// no `.await`. The inner `Arc<String>` lets callers cheaply
    /// snapshot a stable view of the id even if a `/clear` lands
    /// mid-call.
    session_id: ArcSwap<String>,
    /// Per-project filesystem layout — resolves the canonical
    /// `<projectDir>/<sessionId>/session-memory/summary.md` path.
    project_paths: Arc<ProjectPaths>,
    config: MemoryConfig,
    agent: crate::service::extract::AgentSlot,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
    active_shell_tool: ActiveShellTool,
    state: Arc<Mutex<SessionState>>,
    /// In-memory text cache. Empty string ⇒ no extract yet / file missing.
    /// Populated by [`Self::load_from_disk`] at session start and
    /// refreshed after every successful extract. Compact's SM-first
    /// short-circuit reads this without touching disk.
    text_cache: tokio::sync::RwLock<String>,
    /// Sync atomic backing for the in-flight flag — see crate-level
    /// docs for the cancellation-safety rationale.
    in_progress: Arc<AtomicBool>,
    /// Watch channel carrying the `in_progress` value. Pulsed on every
    /// transition (set + clear). Replaces the previous `Notify`,
    /// eliminating the notify-after-check race in
    /// [`Self::wait_for_extraction`].
    in_progress_tx: Arc<watch::Sender<bool>>,
    in_progress_rx: watch::Receiver<bool>,
}

impl std::fmt::Debug for SessionMemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionMemoryService")
            .field("session_id", &*self.session_id.load())
            .field("file_path", &self.file_path())
            .field(
                "session_memory_enabled",
                &self.config.session_memory_enabled,
            )
            .field("in_progress", &self.in_progress.load(Ordering::Acquire))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMemoryOutcome {
    Skipped(SkipReason),
    Completed { duration_ms: i64 },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    InProgress,
    BelowInitThreshold,
    BelowUpdateThreshold,
    NeitherToolCallsNorBreak,
}

impl SessionMemoryService {
    pub fn new(
        project_paths: Arc<ProjectPaths>,
        session_id: String,
        config: MemoryConfig,
        agent: AgentHandleRef,
    ) -> Self {
        Self::with_shared_agent(
            project_paths,
            session_id,
            config,
            Arc::new(std::sync::RwLock::new(agent)),
            Arc::new(NoopEmitter),
            ActiveShellTool::Disabled,
        )
    }

    /// Shared-cell constructor — used by [`crate::MemoryRuntimeBuilder`]
    /// so all three services see the same swappable handle.
    pub fn with_shared_agent(
        project_paths: Arc<ProjectPaths>,
        session_id: String,
        config: MemoryConfig,
        agent: crate::service::extract::AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
        active_shell_tool: ActiveShellTool,
    ) -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            session_id: ArcSwap::from_pointee(session_id),
            project_paths,
            config,
            agent,
            telemetry,
            active_shell_tool,
            state: Arc::new(Mutex::new(SessionState::default())),
            text_cache: tokio::sync::RwLock::new(String::new()),
            in_progress: Arc::new(AtomicBool::new(false)),
            in_progress_tx: Arc::new(tx),
            in_progress_rx: rx,
        }
    }

    /// Try to atomically claim the `in_progress` slot. Returns a Drop
    /// guard on success, `None` if a fork is already running.
    fn try_claim(&self) -> Option<InProgressGuard> {
        match self
            .in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                let _ = self.in_progress_tx.send_replace(true);
                Some(InProgressGuard {
                    flag: self.in_progress.clone(),
                    state: self.state.clone(),
                    notifier: self.in_progress_tx.clone(),
                })
            }
            Err(_) => None,
        }
    }

    /// Resolved on-disk path of the session-memory summary file.
    /// Re-computed each call against the current session id so
    /// `/clear` regen lands subsequent reads / writes in the new
    /// session's directory. Layout:
    /// `<projectDir>/<sessionId>/session-memory/summary.md`.
    pub fn file_path(&self) -> PathBuf {
        let id = self.read_session_id();
        self.project_paths.session_memory_summary(&id)
    }

    /// Update the session id used for disk paths. Called by
    /// `MemoryRuntime::reset` / `SessionRuntime::clear_conversation`
    /// after `regenerateSessionId`. Resets atomically across
    /// `session_id`, `state`, and `text_cache` — a concurrent
    /// `maybe_extract` either sees the pre-reset state (and bails on
    /// the in-progress wait) or the post-reset state (and runs against
    /// the new session id with a clean cursor).
    ///
    /// Best-effort fence: we wait up to 5 s for an in-flight extract
    /// to settle so its post-spawn state write lands against the OLD
    /// session id before we mutate. If the wait times out we proceed
    /// — `InProgressGuard::drop` clears the flag eventually regardless.
    pub async fn set_session_id(&self, new_id: String) {
        let _ = self.wait_for_extraction(Duration::from_secs(5)).await;
        // Hold the state mutex across the cache + session_id mutation
        // so an observer that locks the state can't see a half-reset
        // (cache cleared, state stale) or (state cleared, cache stale).
        let mut state = self.state.lock().await;
        let mut cache = self.text_cache.write().await;
        self.session_id.store(Arc::new(new_id));
        cache.clear();
        *state = SessionState::default();
    }

    /// Best-effort warm of the text cache from disk. Call once at
    /// session start (post-construction) so the SM-first compact
    /// short-circuit can read the cached body without a disk hit.
    /// Missing file ⇒ cache stays empty.
    pub async fn load_from_disk(&self) {
        let path = self.file_path();
        if let Ok(body) = tokio::fs::read_to_string(&path).await {
            *self.text_cache.write().await = body;
        }
    }

    /// Cached session-memory body. Empty string ⇒ no extract yet /
    /// file missing. Compact's SM-first short-circuit reads this
    /// to decide whether to dispatch full LLM summarization or
    /// reuse the pre-extracted SM body.
    pub async fn current_text(&self) -> String {
        self.text_cache.read().await.clone()
    }

    /// Wipe in-memory state and text cache after a compaction
    /// completes (`clearAfterCompact` semantics). The on-disk file is
    /// left alone; the next extract overwrites it section-by-section
    /// via the forked-agent Edit pass.
    ///
    /// Atomic across `state` + `text_cache` for the same reason as
    /// [`Self::set_session_id`].
    pub async fn clear_after_compact(&self) {
        let mut state = self.state.lock().await;
        let mut cache = self.text_cache.write().await;
        cache.clear();
        state.initialized = false;
        state.last_extraction_tokens = 0;
        state.last_extraction_tool_calls = 0;
        state.last_extraction_message_uuid = None;
        state.last_summarized_message_uuid = None;
    }

    /// Override the safely-summarized cursor manually — used by the
    /// SM-first compact path to anchor the kept-tail boundary after
    /// a compact-and-keep-tail write (`setLastSummarizedMessageId`).
    pub async fn set_last_summarized_message_id(&self, uuid: Option<Uuid>) {
        self.state.lock().await.last_summarized_message_uuid = uuid.map(|u| u.to_string());
    }

    /// Decide whether to fire a session-memory update.
    ///
    /// `tool_calls_since_last_extraction` mirrors
    /// `countToolCallsSince(messages, lastMemoryMessageUuid)` —
    /// **cumulative** across all turns since the last successful
    /// extraction (or session start), not just the last assistant
    /// turn. The engine computes this by walking from
    /// [`Self::last_extraction_message_id`].
    /// `had_tool_calls_in_last_turn` is the natural-break signal
    /// (`hasToolCallsInLastAssistantTurn`). When the last assistant
    /// turn used no tools, extraction can fire even when the cumulative
    /// tool-call gate hasn't met threshold. `last_message_id` is advanced
    /// into the cursor on a successful gate pass so the next call's
    /// cumulative count starts from the right boundary.
    pub async fn maybe_extract(
        &self,
        current_tokens: i64,
        tool_calls_since_last_extraction: i32,
        had_tool_calls_in_last_turn: bool,
        last_message_id: Option<String>,
    ) -> SessionMemoryOutcome {
        if !self.config.session_memory_enabled {
            return SessionMemoryOutcome::Skipped(SkipReason::Disabled);
        }
        // Cheap in-progress probe before the mutex.
        if self.in_progress.load(Ordering::Acquire) {
            return SessionMemoryOutcome::Skipped(SkipReason::InProgress);
        }
        let start = Instant::now();
        // Gate + claim — all under the mutex so a concurrent caller
        // observing in_progress=false either passes its own gate AND
        // wins the CAS, or fails the CAS and sees Skipped(InProgress).
        let guard = {
            let mut state = self.state.lock().await;
            if !state.initialized {
                if current_tokens < self.config.session_memory_init_tokens {
                    return SessionMemoryOutcome::Skipped(SkipReason::BelowInitThreshold);
                }
                // The tool-call / natural-break disjunction applies to
                // EVERY extraction, init included — not just updates.
                // Once the init-token threshold is met we still require
                // (tool calls ≥ threshold) OR (no tool calls in the last
                // turn) before firing.
                let tool_call_gate =
                    tool_calls_since_last_extraction >= self.config.session_memory_tool_calls;
                let natural_break = !had_tool_calls_in_last_turn;
                if !tool_call_gate && !natural_break {
                    return SessionMemoryOutcome::Skipped(SkipReason::NeitherToolCallsNorBreak);
                }
                // Flip `initialized` synchronously at gate-pass,
                // independent of the fork outcome — so a failed init
                // fork doesn't re-arm the init-token gate.
                state.initialized = true;
            } else {
                let token_growth = current_tokens - state.last_extraction_tokens;
                if token_growth < self.config.session_memory_update_tokens {
                    return SessionMemoryOutcome::Skipped(SkipReason::BelowUpdateThreshold);
                }
                let tool_call_gate =
                    tool_calls_since_last_extraction >= self.config.session_memory_tool_calls;
                let natural_break = !had_tool_calls_in_last_turn;
                if !tool_call_gate && !natural_break {
                    return SessionMemoryOutcome::Skipped(SkipReason::NeitherToolCallsNorBreak);
                }
            }
            // Gate passed. Claim atomically — try_claim is the only
            // path that flips in_progress, so the mutex guarantees
            // we don't race a concurrent maybe_extract here.
            let Some(guard) = self.try_claim() else {
                return SessionMemoryOutcome::Skipped(SkipReason::InProgress);
            };
            state.extraction_started_at = Some(start);
            if let Some(id) = &last_message_id {
                state.last_extraction_message_uuid = Some(id.clone());
            }
            guard
        };
        tracing::info!(
            current_tokens,
            tool_calls_since_last_extraction,
            had_tool_calls_in_last_turn,
            "session-memory extract dispatch"
        );
        let outcome = self
            .run_fork(
                start,
                current_tokens,
                tool_calls_since_last_extraction,
                last_message_id,
                had_tool_calls_in_last_turn,
                coco_types::ForkLabel::SessionMemoryAuto,
            )
            .await;
        drop(guard);
        outcome
    }

    /// Force a fresh extraction regardless of gates — `/summary`.
    ///
    /// Unlike the prior unconditional version, this gates on
    /// `in_progress` (via `wait_for_extraction`) before claiming.
    /// Two parallel forces (or force + auto) can't race over the SM
    /// file — the second arrival waits, then claims.
    pub async fn force(
        &self,
        current_tokens: i64,
        last_message_id: Option<String>,
        had_tool_calls_in_last_turn: bool,
    ) -> SessionMemoryOutcome {
        if !self.config.session_memory_enabled {
            return SessionMemoryOutcome::Skipped(SkipReason::Disabled);
        }
        // Wait for any in-flight auto-extract before claiming the
        // slot. Bounded by DEFAULT_WAIT_TIMEOUT so a stuck primary
        // can't wedge `/summary` forever.
        let _ = self.wait_for_extraction(DEFAULT_WAIT_TIMEOUT).await;
        let Some(guard) = self.try_claim() else {
            return SessionMemoryOutcome::Skipped(SkipReason::InProgress);
        };
        let start = Instant::now();
        {
            let mut state = self.state.lock().await;
            state.extraction_started_at = Some(start);
        }
        self.telemetry
            .emit(MemoryEvent::SessionMemoryManualExtraction);
        let outcome = self
            .run_fork(
                start,
                current_tokens,
                0,
                last_message_id,
                had_tool_calls_in_last_turn,
                coco_types::ForkLabel::SessionMemoryManual,
            )
            .await;
        drop(guard);
        outcome
    }

    /// Cursor uuid the engine should walk from when computing
    /// cumulative tool-call counts for the next [`Self::maybe_extract`]
    /// call. `None` until the first successful extraction.
    pub async fn last_extraction_message_id(&self) -> Option<String> {
        self.state.lock().await.last_extraction_message_uuid.clone()
    }

    /// Last "safely summarized" message UUID as a String.
    pub async fn last_summarized_message_id(&self) -> Option<String> {
        self.state.lock().await.last_summarized_message_uuid.clone()
    }

    /// Uuid-typed accessor — convenience for compact callers that
    /// hold the cursor as `Option<Uuid>`.
    pub async fn last_summarized_message_uuid(&self) -> Option<Uuid> {
        self.state
            .lock()
            .await
            .last_summarized_message_uuid
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
    }

    /// Whether the SM file currently holds nothing but the seed template.
    pub async fn is_empty(&self) -> bool {
        let template = self.load_template().await;
        match tokio::fs::read_to_string(self.file_path()).await {
            Ok(content) => content.trim() == template.trim(),
            Err(_) => true,
        }
    }

    /// Read the optional template override from
    /// `<session-memory-dir>/config/template.md`. Falls back to the
    /// static 9-section default on ENOENT or empty file.
    async fn load_template(&self) -> String {
        if let Some(parent) = self.file_path().parent() {
            let path = parent.join("config").join("template.md");
            if let Ok(s) = tokio::fs::read_to_string(&path).await
                && !s.trim().is_empty()
            {
                return s;
            }
        }
        build_session_memory_template().to_string()
    }

    async fn load_prompt_override(&self) -> Option<String> {
        let p = self.file_path();
        let parent = p.parent()?;
        let path = parent.join("config").join("prompt.md");
        let s = tokio::fs::read_to_string(&path).await.ok()?;
        if s.trim().is_empty() { None } else { Some(s) }
    }

    /// Wait up to `timeout` (default 15s) for an in-flight extraction
    /// to finish. Returns false on timeout. Past 60s the extraction is
    /// considered stale and we stop waiting.
    ///
    /// Driven by a watch channel — edge-triggered + remembers the
    /// latest value, so a transition that fires between
    /// `borrow_and_update()` and `changed().await` is still observed
    /// on the next iteration. The previous `Notify`-based impl could
    /// lose a notify that arrived during the gap and was forced to
    /// fall back on a 1 s belt-and-braces poll.
    pub async fn wait_for_extraction(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut rx = self.in_progress_rx.clone();
        loop {
            let in_progress = *rx.borrow_and_update();
            if !in_progress {
                return true;
            }
            // Stale extraction check — the in-progress flag may have
            // been leaked by some path that bypasses the Drop guard
            // (shouldn't happen with the current impl, but defensive).
            let started_at = self.state.lock().await.extraction_started_at;
            if let Some(t) = started_at
                && t.elapsed() > STALE_THRESHOLD
            {
                return false;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            // 1 s ceiling per iteration so the stale-extraction probe
            // still fires periodically even if the watch loses a
            // notify under runtime shutdown.
            let wakeup = remaining.min(Duration::from_secs(1));
            if tokio::time::timeout(wakeup, rx.changed()).await.is_err() {
                // Either timed out our iteration window, or the
                // sender was dropped. Loop body will re-check.
                continue;
            }
        }
    }

    /// Read the session-memory file from disk, truncating each section
    /// to the per-section token budget. Returns `None` when the file
    /// doesn't exist yet (no extraction has fired).
    pub async fn current_content(&self) -> Option<String> {
        let raw = tokio::fs::read_to_string(self.file_path()).await.ok()?;
        self.telemetry.emit(MemoryEvent::SessionMemoryLoaded {
            content_length: raw.len() as i64,
        });
        Some(truncate_session_memory_for_compact(
            &raw,
            self.config.session_memory_per_section_tokens,
        ))
    }

    /// Stateless truncation pass-through, exposed so compact glue can
    /// use it on already-loaded content.
    pub fn truncate_for_compact(&self, content: &str) -> String {
        truncate_session_memory_for_compact(content, self.config.session_memory_per_section_tokens)
    }

    /// Wipe per-conversation state. Called from `MemoryRuntime::reset`
    /// on `/clear`.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        let mut cache = self.text_cache.write().await;
        *state = SessionState::default();
        cache.clear();
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_fork(
        &self,
        start: Instant,
        current_tokens: i64,
        tool_calls_since_last_extraction: i32,
        last_message_id: Option<String>,
        had_tool_calls_in_last_turn: bool,
        fork_label: coco_types::ForkLabel,
    ) -> SessionMemoryOutcome {
        let file_path = self.file_path();
        let session_id_for_logs = self.read_session_id();

        // Ensure parent dir exists. Use 0o700 for the dir, 0o600
        // for the file — session memory contains a structured summary
        // of the conversation (potentially sensitive).
        if let Some(parent) = file_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return SessionMemoryOutcome::Failed {
                    reason: format!("create session-memory dir: {e}"),
                };
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await
                {
                    tracing::warn!(
                        target: "coco_memory::session",
                        error = %e,
                        path = %parent.display(),
                        "failed to chmod 0o700 session-memory dir — surface may be readable to other users"
                    );
                    self.telemetry.emit(MemoryEvent::SessionMemoryPermsFailed {
                        path: parent.display().to_string(),
                    });
                }
            }
        }

        // Seed the file with the 9-section template if missing.
        let template = self.load_template().await;
        let current = match self.seed_if_missing(&file_path, &template).await {
            Ok(body) => body,
            Err(e) => {
                return SessionMemoryOutcome::Failed { reason: e };
            }
        };

        // Optional user-provided prompt template override.
        let prompt_override = self.load_prompt_override().await;
        let prompt = build_session_memory_update_prompt(
            &current,
            &file_path,
            prompt_override.as_deref(),
            self.config.session_memory_per_section_tokens,
            self.config.session_memory_total_tokens,
        );
        self.telemetry.emit(MemoryEvent::SessionMemoryFileRead {
            content_length: current.len() as i64,
        });
        // Synthetic AgentDefinition pinning `ModelRole::Memory`. See
        // `extract.rs` for the design rationale. Single-source-of-truth:
        // model routing flows through `AgentDefinition.model_role`;
        // memory forks construct an in-process synthetic def at spawn
        // time.
        let memory_def = std::sync::Arc::new(coco_types::AgentDefinition {
            agent_type: coco_types::AgentTypeId::Custom("memory-internal".into()),
            name: "memory-internal".into(),
            model_role: Some(ModelRole::Memory),
            ..Default::default()
        });
        let request = AgentSpawnRequest {
            prompt,
            description: Some("session memory update".into()),
            session_id: self.read_session_id(),
            subagent_type: Some("general-purpose".into()),
            definition: Some(memory_def),
            constraints: Some(AgentSpawnConstraints {
                // Section-by-section edits can legitimately span more
                // turns than the original `Some(3)` allowed —
                // tight cap silently truncated SM updates for models
                // that prefer one-section-per-turn pacing.
                max_turns: Some(self.config.extraction_max_turns.max(5)),
                allowed_write_roots: file_path
                    .parent()
                    .map(|p| vec![p.to_path_buf()])
                    .unwrap_or_default(),
            }),
            skip_transcript: true,
            // `canUseTool: createSessionMemCanUseTool(memoryPath)`.
            can_use_tool: Some(crate::can_use_tool::create_session_mem_handle(
                file_path.clone(),
            )),
            require_can_use_tool: false,
            fork_label: Some(fork_label),
            active_shell_tool: self.active_shell_tool,
            ..Default::default()
        };

        tracing::info!(
            target: "coco_memory::session",
            session_id = %session_id_for_logs,
            current_tokens,
            tool_calls_since_last_extraction,
            "spawning session-memory update subagent"
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
                    target: "coco_memory::session",
                    session_id = %session_id_for_logs,
                    duration_ms,
                    input_tokens = resp.input_tokens,
                    output_tokens = resp.output_tokens,
                    "session-memory update complete"
                );
                self.telemetry.emit(MemoryEvent::SessionMemoryExtracted {
                    input_tokens: resp.input_tokens,
                    output_tokens: resp.output_tokens,
                    cache_read_tokens: resp.cache_read_tokens,
                    cache_creation_tokens: resp.cache_creation_tokens,
                    duration_ms,
                });
                {
                    // `state.initialized` is flipped synchronously at the
                    // gate-pass point in `maybe_extract`
                    // (`markSessionMemoryInitialized`), not here — a failed
                    // fork must NOT re-arm the init-token gate.
                    let mut state = self.state.lock().await;
                    state.last_extraction_tokens = current_tokens;
                    state.last_extraction_tool_calls = tool_calls_since_last_extraction;
                    // Advance the "safely summarized" cursor only when
                    // the prior assistant turn had no tool calls
                    // (`updateLastSummarizedMessageIdIfSafe`).
                    if !had_tool_calls_in_last_turn && let Some(id) = last_message_id {
                        state.last_summarized_message_uuid = Some(id);
                    }
                }
                // Refresh the in-memory text cache so the next
                // SM-first compact short-circuit reads the freshly
                // written body without a disk hit.
                if let Ok(new_text) = tokio::fs::read_to_string(&file_path).await {
                    *self.text_cache.write().await = new_text;
                }
                SessionMemoryOutcome::Completed { duration_ms }
            }
            Err(e) => {
                tracing::warn!(
                    target: "coco_memory::session",
                    session_id = %session_id_for_logs,
                    error = %e,
                    "session-memory update failed"
                );
                SessionMemoryOutcome::Failed { reason: e }
            }
        }
    }

    fn read_session_id(&self) -> String {
        (**self.session_id.load()).clone()
    }

    /// Atomically seed the session-memory file with `template` if it
    /// doesn't exist, then read back the current content.
    async fn seed_if_missing(
        &self,
        file_path: &std::path::Path,
        template: &str,
    ) -> std::result::Result<String, String> {
        let path = file_path.to_path_buf();
        let body = template.to_string();
        let seed_result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(mut f) => {
                    f.write_all(body.as_bytes())?;
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(e),
            }
        })
        .await;
        match seed_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("write session-memory seed: {e}")),
            Err(e) => return Err(format!("spawn_blocking failed: {e}")),
        }
        tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| format!("read session-memory after seed: {e}"))
    }
}

/// Default retention window for orphan session-memory cleanup —
/// 30 days, mirroring [`coco_coordinator::worktree::AgentWorktreeManager::cleanup_stale`].
pub const DEFAULT_SM_RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 30);

/// Sweep abandoned per-session SM files under `<project_dir>/`.
pub async fn cleanup_stale_session_memories(
    project_dir: &std::path::Path,
    active_session_id: &str,
    older_than: Duration,
) -> std::io::Result<i32> {
    let mut entries = match tokio::fs::read_dir(project_dir).await {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };

    let now = SystemTime::now();
    let mut removed = 0i32;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = match entry.file_type().await {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }

        let session_dir = entry.path();
        let session_name = match session_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if session_name == active_session_id {
            continue;
        }

        let sm_dir = session_dir.join("session-memory");
        let summary = sm_dir.join("summary.md");

        let modified = match tokio::fs::metadata(&summary).await {
            Ok(m) => m.modified().ok(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => tokio::fs::metadata(&sm_dir)
                .await
                .ok()
                .and_then(|m| m.modified().ok()),
            Err(_) => continue,
        };
        let Some(modified) = modified else {
            continue;
        };

        match now.duration_since(modified) {
            Ok(age) if age >= older_than => match tokio::fs::remove_dir_all(&sm_dir).await {
                Ok(()) => {
                    let age_days: u64 = age.as_secs() / 86_400;
                    tracing::debug!(
                        target: "coco_memory::session::cleanup",
                        path = %sm_dir.display(),
                        age_days,
                        "removed orphan session-memory dir"
                    );
                    removed += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(
                        target: "coco_memory::session::cleanup",
                        path = %sm_dir.display(),
                        error = %err,
                        "failed to remove orphan session-memory dir"
                    );
                }
            },
            _ => continue,
        }
    }

    Ok(removed)
}

#[cfg(test)]
#[path = "session.test.rs"]
mod tests;
