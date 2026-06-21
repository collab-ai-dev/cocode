//! Turn-end memory extraction service.
//!
//! After every eligible turn, fork a subagent with a 5-turn cap and
//! a memdir-only write fence. The agent reads existing memories
//! (manifest pre-injected into its prompt), then writes / edits
//! memory files based on the conversation slice since the last cursor.
//!
//! State machine:
//! - throttle gate (every Nth turn)
//! - mutual exclusion (don't run while in-flight; stash + trailing run)
//! - skip-if-main-already-wrote (`has_memory_writes_since`)
//! - cursor advance after success
//!
//! ## Cancellation safety
//!
//! Without a Drop guard the `in_progress` flag stays `true` for the
//! rest of the session, wedging every subsequent `maybe_extract` call
//! into `Skipped(InProgress)` until process restart.
//!
//! The fix: hold `in_progress` in a sync `Arc<AtomicBool>` and wrap
//! it in [`InProgressGuard`] (RAII). The guard's `Drop` synchronously
//! clears the flag, so a cancelled `maybe_extract` future can't leak
//! the flag. The atomic also lets `wait_for_in_progress_clear` (and
//! the watch channel signalling) read state without an `.await`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use arc_swap::ArcSwap;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnConstraints;
use coco_tool_runtime::AgentSpawnRequest;
use coco_types::ActiveShellTool;
use coco_types::ModelRole;
use coco_types::messages::Message;
use tokio::sync::Mutex;
use tokio::sync::watch;

use crate::config::MemoryConfig;
use crate::prompt::build_extract_prompt;
use crate::scan;
use crate::telemetry::MemoryEvent;
use crate::telemetry::MemoryTelemetryEmitter;
use crate::telemetry::NoopEmitter;

/// Drain timeout on shutdown (60 s).
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(60);

/// Max consecutive `Failed` outcomes before the throttle ceiling
/// starts doubling per failure. Caps at `1 << MAX_BACKOFF_SHIFT`
/// multiplier of the configured `extraction_throttle`. Prevents an
/// `install_agent`-delayed SDK session from burning one fork per N
/// turns forever.
const MAX_BACKOFF_SHIFT: u32 = 5;

/// Cross-turn extraction state. Separates the `in_progress` flag (now
/// an atomic with a Drop-safe guard) from the rest of the bookkeeping.
#[derive(Default)]
struct ExtractState {
    /// Last message UUID that's been folded into an extraction. The
    /// extraction analyzes only messages newer than this.
    last_cursor: Option<String>,
    /// Throttle counter — increments per eligible turn, resets on fire.
    turns_since_last: i32,
    /// Consecutive `Failed` outcomes — drives exponential backoff so a
    /// delayed `install_agent` doesn't burn one fork per N turns.
    consecutive_failures: u32,
    /// Latest stashed `TurnInput` queued during an in-flight run.
    /// The trailing run uses **the most recent** stashed context —
    /// overwriting prior stashes — so it picks up messages that
    /// arrived during the primary run rather than re-running on the
    /// same stale slice. Carrying the full `TurnInput` here means
    /// the closure (`fork_messages`) is captured at stash time, so
    /// it serializes the latest history when invoked.
    pending_trailing: Option<TurnInput>,
}

impl std::fmt::Debug for ExtractState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractState")
            .field("last_cursor", &self.last_cursor)
            .field("turns_since_last", &self.turns_since_last)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("pending_trailing", &self.pending_trailing.is_some())
            .finish()
    }
}

/// Shared swappable cell for the agent handle. The runtime owns the
/// master `Arc<RwLock<...>>` and hands clones to every service so a
/// later `MemoryRuntime::install_agent(handle)` propagates atomically
/// — no need to rebuild service instances.
///
/// `std::sync::RwLock` (not `tokio::sync::RwLock`) because reads are
/// "clone the inner `Arc` and drop the guard immediately" — no await
/// across the guard, no need for an async-aware lock, no futex hop
/// onto the runtime. `arc_swap::ArcSwapAny<Arc<dyn AgentHandle>>`
/// would be even cheaper but DSTs (`dyn AgentHandle`) don't satisfy
/// arc-swap's `RefCnt: Sized` bound; RwLock is the next-best primitive.
pub type AgentSlot = Arc<std::sync::RwLock<AgentHandleRef>>;

/// RAII guard for the `in_progress` flag. Constructed only after a
/// successful CAS from `false → true`; `Drop` synchronously stores
/// `false` and pulses the watch channel.
///
/// Holds an `Arc` clone so dropping the parent `ExtractService` mid-
/// fork still leaves the guard valid (extremely rare — the runtime
/// outlives the fork in normal use — but defensive).
struct InProgressGuard {
    flag: Arc<AtomicBool>,
    notifier: Arc<watch::Sender<bool>>,
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        // Release first so any concurrent CAS observer sees the
        // cleared state before the watch wakes.
        self.flag.store(false, Ordering::Release);
        // `send_replace` is infallible (we own the only `Sender`'s
        // Arc); broadcasts to every waiter that
        // in_progress just transitioned. A dropped receiver is fine.
        let _ = self.notifier.send_replace(false);
    }
}

/// Turn-end extraction service.
pub struct ExtractService {
    session_id: ArcSwap<String>,
    memory_dir: PathBuf,
    config: MemoryConfig,
    agent: AgentSlot,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
    active_shell_tool: ActiveShellTool,
    /// User-visible save notices land here on a successful
    /// extraction; the engine drains it once per turn and injects a
    /// `SystemMemorySavedMessage` into history.
    notices: crate::notice::NoticeInbox,
    /// State guarded by an async mutex — cursor + throttle counter +
    /// pending-trailing slot. Excludes `in_progress` (see below).
    state: Mutex<ExtractState>,
    /// `in_progress` lives in a sync atomic so the RAII Drop guard can
    /// clear it without `.await`. A cancelled `maybe_extract` future
    /// gets cleaned up by the guard's `Drop`.
    in_progress: Arc<AtomicBool>,
    /// Watch channel carrying the current `in_progress` value. Eliminates
    /// the notify-after-check race in the polling `drain` path:
    /// `Receiver::changed()` is edge-triggered AND remembers the latest
    /// value, so a transition that fires between `borrow_and_update()`
    /// and `changed().await` is still observed on the next call.
    in_progress_tx: Arc<watch::Sender<bool>>,
    in_progress_rx: watch::Receiver<bool>,
}

impl std::fmt::Debug for ExtractService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractService")
            .field("memory_dir", &self.memory_dir)
            .field("extraction_enabled", &self.config.extraction_enabled)
            .field("in_progress", &self.in_progress.load(Ordering::Acquire))
            .finish()
    }
}

/// One per-turn outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractOutcome {
    /// Extraction wasn't fired this turn.
    Skipped(SkipReason),
    /// Extraction fired and completed (synchronous wait).
    Completed { duration_ms: i64 },
    /// Extraction fired and failed.
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    InProgress,
    Throttled,
    DirectWrite,
    BackoffActive,
}

/// Boxed lazy builder for the message slice the extraction agent
/// analyzes. Invoked **only** when all gates pass and the service is
/// actually about to spawn — turns where extraction is skipped
/// (throttled, in-flight, direct-write) avoid per-message allocations
/// entirely.
///
/// Returns `Vec<Arc<Message>>` so the in-process spawn path shares
/// the parent's authoritative history allocations directly — no
/// serialize/deserialize round-trip across the trait boundary.
pub type LazyForkMessages = Box<dyn FnOnce() -> Vec<Arc<Message>> + Send>;

/// Lazy predicate for the "main agent wrote to memdir since the
/// cursor" check. Evaluated at gate time so trailing runs — which
/// fire after a primary run completes and may see new direct-writes
/// that landed during the primary's window — get a fresh answer
/// instead of the stale snapshot from when this `TurnInput` was first
/// constructed. Re-evaluated by every entry into the run path, not
/// cached.
pub type HasMemoryWritesFn = Box<dyn FnOnce() -> bool + Send>;

/// Per-turn input: a lazy slice builder + turn-level signals.
///
/// `has_memory_writes` is a **closure** rather than a boolean so the
/// trailing run can re-check it against history that grew during the
/// primary's window. Without this, a main-agent direct-write that
/// landed mid-primary would slip past the trailing run's mutual-
/// exclusion gate.
pub struct TurnInput {
    pub fork_messages: LazyForkMessages,
    pub message_count: i32,
    pub last_message_id: Option<String>,
    pub has_memory_writes: HasMemoryWritesFn,
}

impl Default for TurnInput {
    fn default() -> Self {
        Self {
            fork_messages: Box::new(Vec::new),
            message_count: 0,
            last_message_id: None,
            has_memory_writes: Box::new(|| false),
        }
    }
}

impl ExtractService {
    pub fn new(memory_dir: PathBuf, config: MemoryConfig, agent: AgentHandleRef) -> Self {
        Self::with_shared_agent(
            memory_dir,
            config,
            Arc::new(std::sync::RwLock::new(agent)),
            Arc::new(NoopEmitter),
        )
    }

    /// Shared-cell constructor — used by the builder so all three
    /// services see the same swappable handle.
    pub fn with_shared_agent(
        memory_dir: PathBuf,
        config: MemoryConfig,
        agent: AgentSlot,
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
        agent: AgentSlot,
        telemetry: Arc<dyn MemoryTelemetryEmitter>,
        notices: crate::notice::NoticeInbox,
        active_shell_tool: ActiveShellTool,
        session_id: String,
    ) -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            session_id: ArcSwap::from_pointee(session_id),
            memory_dir,
            config,
            agent,
            telemetry,
            active_shell_tool,
            notices,
            state: Mutex::new(ExtractState::default()),
            in_progress: Arc::new(AtomicBool::new(false)),
            in_progress_tx: Arc::new(tx),
            in_progress_rx: rx,
        }
    }

    pub fn set_session_id(&self, new_id: String) {
        self.session_id.store(Arc::new(new_id));
    }

    /// Try to atomically claim the `in_progress` slot. Returns a Drop
    /// guard on success (auto-releases on cancellation), `None` if
    /// another caller already holds the slot.
    fn try_claim(&self) -> Option<InProgressGuard> {
        match self
            .in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                // Notify watchers — they may be parked in `wait_for_idle`.
                let _ = self.in_progress_tx.send_replace(true);
                Some(InProgressGuard {
                    flag: self.in_progress.clone(),
                    notifier: self.in_progress_tx.clone(),
                })
            }
            Err(_) => None,
        }
    }

    /// Compute the effective throttle ceiling, applying exponential
    /// backoff on consecutive failures. Capped at `1 << MAX_BACKOFF_SHIFT`
    /// (32×) so a permanently-failing handle still attempts
    /// `extraction_throttle * 32` turns apart instead of every turn.
    fn effective_throttle(&self, state: &ExtractState) -> i32 {
        let base = self.config.extraction_throttle.max(1);
        let shift = state.consecutive_failures.min(MAX_BACKOFF_SHIFT);
        base.saturating_mul(1i32 << shift)
    }

    /// Run-or-skip decision keyed off [`TurnInput`]. The caller's
    /// `fork_messages` closure is invoked **only** once all gates pass
    /// — skipped turns avoid the per-message JSON serialization.
    pub async fn maybe_extract(&self, input: TurnInput) -> ExtractOutcome {
        if !self.config.extraction_enabled {
            return ExtractOutcome::Skipped(SkipReason::Disabled);
        }

        // Destructure so we can evaluate `has_memory_writes` OUTSIDE
        // the mutex critical section.
        let TurnInput {
            fork_messages,
            message_count,
            last_message_id,
            has_memory_writes,
        } = input;

        // Coalesce-or-fall-through under the mutex. The mutex
        // serializes the in_progress check against the stash write,
        // so a concurrent caller either:
        //   (a) sees in_progress=true and stashes (us, in this
        //       branch), or
        //   (b) wins the CAS in try_claim below.
        // The early-probe + re-check ordering avoids running
        // `has_memory_writes()` (which may walk history) when we're
        // certainly stashing.
        if self.in_progress.load(Ordering::Acquire) {
            let mut state = self.state.lock().await;
            if self.in_progress.load(Ordering::Acquire) {
                state.pending_trailing = Some(TurnInput {
                    fork_messages,
                    message_count,
                    last_message_id,
                    has_memory_writes,
                });
                tracing::debug!(
                    message_count,
                    "auto-memory extract skipped: in progress (queued trailing)"
                );
                self.telemetry.emit(MemoryEvent::ExtractionCoalesced);
                return ExtractOutcome::Skipped(SkipReason::InProgress);
            }
            // Race-loser: in_progress flipped false between the early
            // probe and the lock. Fall through to the regular gate
            // logic below without releasing the state mutex prematurely.
        }

        // Evaluate direct-write outside the mutex (it may walk
        // history). The result is consumed under the lock so a
        // concurrent direct-write that lands mid-evaluation either:
        //  - is observed here and we advance the cursor + skip; or
        //  - is observed by the trailing run's re-check (TurnInput's
        //    `has_memory_writes` is a fresh closure).
        let direct_write = has_memory_writes();

        // Gate + claim — all under the mutex so a concurrent
        // `maybe_extract` either sees `in_progress=true` (and stashes)
        // or sees the post-claim state and steps to its own gate
        // checks. This is the load-bearing critical section.
        let guard = {
            let mut state = self.state.lock().await;
            if direct_write {
                // When the main agent wrote memory directly this turn,
                // skip the fork AND advance the cursor past the range
                // so the next eligible turn doesn't re-consider these
                // already-handled messages.
                if let Some(ref id) = last_message_id {
                    state.last_cursor = Some(id.clone());
                }
                self.telemetry
                    .emit(MemoryEvent::ExtractionSkippedDirectWrite { message_count });
                tracing::debug!(
                    message_count,
                    "auto-memory extract skipped: model wrote memory directly"
                );
                return ExtractOutcome::Skipped(SkipReason::DirectWrite);
            }
            state.turns_since_last += 1;
            let ceiling = self.effective_throttle(&state);
            if state.turns_since_last < ceiling {
                tracing::debug!(
                    turns_since_last = state.turns_since_last,
                    throttle = ceiling,
                    consecutive_failures = state.consecutive_failures,
                    "auto-memory extract skipped: throttled (with backoff)"
                );
                let reason = if state.consecutive_failures > 0 {
                    SkipReason::BackoffActive
                } else {
                    SkipReason::Throttled
                };
                return ExtractOutcome::Skipped(reason);
            }
            state.turns_since_last = 0;
            // Claim the in_progress slot under the same lock so a racing
            // call lands cleanly on the stash path above. If somebody
            // beat us to the claim (extremely rare — would require an
            // out-of-band caller bypassing this gate) treat it as
            // InProgress.
            let Some(guard) = self.try_claim() else {
                state.turns_since_last = ceiling; // rewind the counter
                return ExtractOutcome::Skipped(SkipReason::InProgress);
            };
            guard
        };
        tracing::info!(
            message_count,
            "auto-memory extract dispatch (forking agent)"
        );

        let primary_fork_context = fork_messages();
        let outcome = self.run(message_count, primary_fork_context).await;
        tracing::info!(
            outcome = ?std::mem::discriminant(&outcome),
            "auto-memory extract done"
        );

        // Update cursor + failure counter under the mutex; drop the
        // guard at the very end so a watcher observing `in_progress
        // = false` also observes the cursor/counter updates.
        {
            let mut state = self.state.lock().await;
            match &outcome {
                ExtractOutcome::Completed { .. } => {
                    state.consecutive_failures = 0;
                    if let Some(id) = last_message_id.clone() {
                        state.last_cursor = Some(id);
                    }
                }
                ExtractOutcome::Failed { .. } => {
                    // CLAUDE.md invariant: cursor preserved on failure
                    // so the next eligible turn retries the same range.
                    // Bump consecutive_failures to drive backoff so a
                    // permanently-failing handle stops burning a fork
                    // every turn.
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                }
                ExtractOutcome::Skipped(_) => {
                    // `run` never returns Skipped, but keep the match
                    // exhaustive without resetting counters.
                }
            }
        }
        // Drop the primary's guard now — trailing runs claim their own.
        drop(guard);

        // Drain trailing runs in a loop. Each iteration atomically takes
        // the pending slot AND claims `in_progress` so a fresh
        // `maybe_extract` arriving mid-drain stashes (or no-ops once
        // the slot frees again).
        loop {
            let (trailing_input, trailing_guard) = {
                let mut state = self.state.lock().await;
                let Some(input) = state.pending_trailing.take() else {
                    break;
                };
                let Some(guard) = self.try_claim() else {
                    // The slot is somehow already claimed — re-stash
                    // and let the holder drain when it finishes.
                    state.pending_trailing = Some(input);
                    break;
                };
                (input, guard)
            };
            let TurnInput {
                fork_messages: trailing_fork_messages,
                message_count: trailing_count,
                last_message_id: trailing_last_id,
                has_memory_writes: trailing_has_writes,
            } = trailing_input;

            if trailing_has_writes() {
                let mut state = self.state.lock().await;
                if let Some(id) = trailing_last_id {
                    state.last_cursor = Some(id);
                }
                self.telemetry
                    .emit(MemoryEvent::ExtractionSkippedDirectWrite {
                        message_count: trailing_count,
                    });
                tracing::debug!(
                    message_count = trailing_count,
                    "auto-memory trailing extract skipped: model wrote memory directly"
                );
                // Drop guard; another iteration may pick up a fresh stash.
                drop(trailing_guard);
                continue;
            }

            let trailing_fork_context = trailing_fork_messages();
            let trailing_outcome = self.run(trailing_count, trailing_fork_context).await;
            {
                let mut state = self.state.lock().await;
                match &trailing_outcome {
                    ExtractOutcome::Completed { .. } => {
                        state.consecutive_failures = 0;
                        if let Some(id) = trailing_last_id {
                            state.last_cursor = Some(id);
                        }
                    }
                    ExtractOutcome::Failed { .. } => {
                        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                    }
                    ExtractOutcome::Skipped(_) => {}
                }
            }
            drop(trailing_guard);
        }

        outcome
    }

    /// Force a fresh extraction regardless of throttle / in-progress
    /// flags — bound to a `/extract` slash command (planned). Unlike
    /// the previous unconditional version this gates on `in_progress`
    /// before claiming, so two parallel forces can't race over the
    /// memdir.
    pub async fn force(&self, input: TurnInput) -> ExtractOutcome {
        if !self.config.extraction_enabled {
            return ExtractOutcome::Skipped(SkipReason::Disabled);
        }
        // Wait for any in-flight run before claiming. Bounded so a
        // stuck primary doesn't wedge the force.
        let _ = self.wait_for_idle(DEFAULT_DRAIN_TIMEOUT).await;
        let Some(guard) = self.try_claim() else {
            return ExtractOutcome::Skipped(SkipReason::InProgress);
        };
        // Emit so dashboards can split auto vs manual cadence.
        self.telemetry.emit(MemoryEvent::ExtractionManual);
        let fork_context = (input.fork_messages)();
        let outcome = self.run(input.message_count, fork_context).await;
        // Reset the throttle window — a manual run is the freshest
        // signal we have. Cursor advances only on success, same
        // contract as the auto path.
        {
            let mut state = self.state.lock().await;
            state.turns_since_last = 0;
            match &outcome {
                ExtractOutcome::Completed { .. } => {
                    state.consecutive_failures = 0;
                    if let Some(id) = input.last_message_id {
                        state.last_cursor = Some(id);
                    }
                }
                ExtractOutcome::Failed { .. } => {
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                }
                ExtractOutcome::Skipped(_) => {}
            }
        }
        drop(guard);
        outcome
    }

    /// Wait up to `timeout` for the in-flight slot to clear. Driven by
    /// the watch channel so a notify lost between check and park is
    /// still observed via `borrow_and_update()` on the next iteration.
    /// Returns `true` when idle, `false` on timeout.
    pub async fn wait_for_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut rx = self.in_progress_rx.clone();
        loop {
            if !*rx.borrow_and_update() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            // `changed()` returns Err only if the sender is dropped —
            // we hold an Arc to it on the service, so that's
            // unreachable in practice. Map both cases to the timeout
            // branch so a future refactor that introduces a real
            // shutdown path can't accidentally loop forever.
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                return false;
            }
        }
    }

    /// Wait up to `timeout` for an in-flight extraction to complete.
    /// Used at session shutdown so partial writes don't get lost.
    ///
    /// Alias for [`Self::wait_for_idle`] — kept under the historical
    /// name for callers in the SDK / TUI / headless runners.
    pub async fn drain(&self, timeout: Duration) -> bool {
        self.wait_for_idle(timeout).await
    }

    /// Last cursor (message UUID); call sites use this to compute the
    /// "messages since last extraction" slice for the agent prompt.
    pub async fn last_cursor(&self) -> Option<String> {
        self.state.lock().await.last_cursor.clone()
    }

    /// Wipe per-conversation state (cursor + throttle counter +
    /// pending-trailing flag) — called from `MemoryRuntime::reset`
    /// on `/clear` so the next conversation extracts fresh.
    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        *state = ExtractState::default();
    }

    async fn run(&self, message_count: i32, fork_context: Vec<Arc<Message>>) -> ExtractOutcome {
        let start = Instant::now();
        let manifest = scan::format_memory_manifest(&scan::scan_memory_files(&self.memory_dir));
        let prompt = build_extract_prompt(
            message_count,
            &manifest,
            self.config.skip_index,
            self.config.team_memory_enabled,
        );
        tracing::info!(
            target: "coco_memory::extract",
            message_count,
            fork_context = fork_context.len(),
            max_turns = self.config.extraction_max_turns,
            "spawning extraction subagent"
        );

        // Memory forks need to route to `ModelRole::Memory` so an
        // operator configuring `settings.models.memory` actually
        // steers them. Catalog `general-purpose` has no `model_role`
        // declared → would fall through to `Subagent`. Construct a
        // synthetic `AgentDefinition` carrying the desired role and
        // thread it through `definition` — the coordinator's
        // `resolve_subagent_selection` reads `definition.model_role`
        // as the role source of truth.
        //
        // Single-source-of-truth design: model routing flows through
        // `AgentDefinition` only, never through a per-request override
        // slot. The synthetic def lives in-process at spawn time, not
        // in the persistent catalog.
        let memory_def = std::sync::Arc::new(coco_types::AgentDefinition {
            agent_type: coco_types::AgentTypeId::Custom("memory-internal".into()),
            name: "memory-internal".into(),
            model_role: Some(ModelRole::Memory),
            ..Default::default()
        });
        let request = AgentSpawnRequest {
            prompt,
            description: Some("memory extraction".into()),
            session_id: (**self.session_id.load()).clone(),
            subagent_type: Some("general-purpose".into()),
            definition: Some(memory_def),
            run_in_background: false,
            auto_background_ms: None,
            // No worktree/remote isolation. The "fork" behaviour (child
            // sees the parent's message slice prepended) is driven solely
            // by `fork_context_messages` below — the old `isolation:
            // "fork"` string was never a real `AgentIsolation` variant and
            // matched no spawn-path branch.
            isolation: None,
            fork_context_messages: fork_context,
            constraints: Some(AgentSpawnConstraints {
                max_turns: Some(self.config.extraction_max_turns),
                allowed_write_roots: vec![self.memory_dir.clone()],
            }),
            // The background agent must not record per-message entries
            // to the user's transcript — its tool-uses race the main
            // thread's writer and pollute the JSONL.
            skip_transcript: true,
            // Allows Read/Glob/Grep, read-only Bash, Edit/Write
            // within memory_dir; denies everything else. The
            // canUseTool gate runs at tool-runtime step 3.5,
            // composing with the `allowed_write_roots` fence above
            // (callback = inner ring; field = outer ring).
            can_use_tool: Some(crate::can_use_tool::create_auto_mem_handle_with_telemetry(
                self.memory_dir.clone(),
                self.telemetry.clone(),
            )),
            require_can_use_tool: false,
            fork_label: Some(coco_types::ForkLabel::ExtractMemories),
            active_shell_tool: self.active_shell_tool,
            ..Default::default()
        };

        // Clone the inner `Arc<dyn AgentHandle>` while holding the
        // sync read guard, then drop the guard before any `.await`.
        // `PoisonError::into_inner` recovers from a poisoned lock —
        // poisoning only happens if a prior write panicked, and for
        // an install-only handle a stale-but-readable handle beats
        // crashing the extract path.
        let agent = self
            .agent
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match agent.spawn_agent(request).await {
            Ok(response) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                // Prefer the explicit `paths_written` list when the
                // spawn driver populated it — exclude `MEMORY.md`
                // from the user-facing "Saved" count (the index file
                // is mechanical). Fall back to `tool_use_counts` for
                // legacy / minimal driver impls that haven't wired
                // the path list yet.
                let entrypoint = crate::store::ENTRYPOINT_NAME;
                let topic_paths: Vec<String> = response
                    .paths_written
                    .iter()
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n != entrypoint)
                    })
                    .map(|p| p.display().to_string())
                    .collect();
                let files_written: i32 = if !response.paths_written.is_empty() {
                    topic_paths.len() as i32
                } else {
                    [
                        coco_types::ToolName::Write.as_str(),
                        coco_types::ToolName::Edit.as_str(),
                        coco_types::ToolName::NotebookEdit.as_str(),
                    ]
                    .iter()
                    .map(|t| response.tool_use_counts.get(*t).copied().unwrap_or(0))
                    .sum::<i64>() as i32
                };
                tracing::info!(
                    target: "coco_memory::extract",
                    duration_ms,
                    files_written,
                    turn_count = response.total_tool_use_count,
                    input_tokens = response.input_tokens,
                    output_tokens = response.output_tokens,
                    cache_read = response.cache_read_tokens,
                    cache_create = response.cache_creation_tokens,
                    "extraction complete"
                );
                self.telemetry.emit(MemoryEvent::ExtractionCompleted {
                    turn_count: response.total_tool_use_count as i32,
                    input_tokens: response.input_tokens,
                    output_tokens: response.output_tokens,
                    cache_read_tokens: response.cache_read_tokens,
                    cache_creation_tokens: response.cache_creation_tokens,
                    files_written,
                    duration_ms,
                });
                // Only push a user-visible notice when at least one
                // topic file was written (the index doesn't count).
                if !topic_paths.is_empty() {
                    self.notices.push(crate::notice::MemoryUserNotice {
                        written_paths: topic_paths,
                        verb: crate::notice::NoticeVerb::Saved,
                    });
                }
                ExtractOutcome::Completed { duration_ms }
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                tracing::warn!(
                    target: "coco_memory::extract",
                    error = %e,
                    duration_ms,
                    "extraction subagent failed"
                );
                self.telemetry
                    .emit(MemoryEvent::ExtractionError { duration_ms });
                ExtractOutcome::Failed { reason: e }
            }
        }
    }
}

#[cfg(test)]
#[path = "extract.test.rs"]
mod tests;
