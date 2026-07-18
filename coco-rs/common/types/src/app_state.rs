//! Typed cross-turn shared state carried on `ToolUseContext.app_state`.
//!
//! This struct replaces a previously untyped `serde_json::Value` map. It
//! is shared between `coco-tools` (writers: EnterPlanMode/ExitPlanMode),
//! `coco-query` (reader+writer: PlanModeReminder), and the `coco-cli`
//! driver (writer: ClearConversation + reader: auto-title gate).
//!
//! `appState.toolPermissionContext` in `state/AppStateStore.ts`.
//! TS keeps the live permission-mode + plan-mode latches on a single
//! shared-mutable store; readers call `getAppState()` fresh and writers
//! use `setAppState(prev => ...)` to mutate. Rust mirrors this via
//! `Arc<RwLock<ToolAppState>>` on the engine + every tool context.
//!
//! All fields are plain value types so `Default` produces the initial
//! empty state; adding a field is a one-line edit here, not a string key
//! coordination across three crates.

use crate::AdditionalWorkingDir;
use crate::AgentColorName;
use crate::ExitPlanModeOutcome;
use crate::PermissionMode;
use crate::PermissionRuleSource;
use crate::PermissionRulesBySource;
use crate::RateLimitEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use tokio::sync::RwLock;

/// TS-shaped pending plan verification state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingPlanVerificationState {
    pub plan: String,
    pub verification_started: bool,
    pub verification_completed: bool,
}

impl PendingPlanVerificationState {
    pub fn new(plan: String) -> Self {
        Self {
            plan,
            verification_started: false,
            verification_completed: false,
        }
    }

    pub fn needs_reminder(&self) -> bool {
        !self.verification_started && !self.verification_completed
    }
}

/// Live source of truth for the permission-context BASE that
/// `ToolContextFactory::build` snapshots each batch in one read-lock acquire
/// (`guard.permissions.clone()`). Shared by `Arc` across the main engine and
/// its subagents/forks, so subagents read-through the parent's live rules.
/// Mutated only by the main-session apply path
/// (`apply_permission_updates_everywhere`); subagents never write it.
/// Data only — all permission *logic* (apply/strip/merge) lives in
/// `coco_permissions`, which `common/types` must not depend on.
#[derive(Debug, Clone, Default)]
pub struct LiveToolPermissionState {
    /// `None` = uninitialized; seeded to `Some(config.permission_mode)` at
    /// bootstrap. Readers fall back to the config mode while `None`.
    pub mode: Option<PermissionMode>,
    /// Mode active before entering plan mode. TS: `prePlanMode`.
    pub pre_plan_mode: Option<PermissionMode>,
    /// Dangerous allow rules stashed while Auto mode is active, restored on
    /// exit. TS: `strippedDangerousRules`.
    pub stripped_dangerous_rules: Option<PermissionRulesBySource>,
    /// Session allow/deny/ask rules. TS: `alwaysAllowRules` / `alwaysDenyRules`
    /// / `alwaysAskRules`.
    pub allow_rules: PermissionRulesBySource,
    pub deny_rules: PermissionRulesBySource,
    pub ask_rules: PermissionRulesBySource,
    /// Read-scope working dirs beyond cwd. TS: `additionalWorkingDirectories`.
    pub additional_dirs: HashMap<String, AdditionalWorkingDir>,
    /// Source-specific roots for path-scoped file rules. TS:
    /// `rootPathForSource`.
    pub permission_rule_source_roots: HashMap<PermissionRuleSource, PathBuf>,
}

impl LiveToolPermissionState {
    /// Resolve the effective mode, falling back when uninitialized.
    pub fn mode_or(&self, fallback: PermissionMode) -> PermissionMode {
        self.mode.unwrap_or(fallback)
    }
}

/// Cross-turn shared state carried on `ToolUseContext.app_state`.
/// Grouped by lifecycle:
/// - **Live permission mode** (`permission_mode`, `pre_plan_mode`,
/// `stripped_dangerous_rules`) — source of truth for mode-dependent
/// decisions. `appState.toolPermissionContext.{mode,
/// prePlanMode, strippedDangerousRules}`. Rebuilt into
/// `ToolUseContext.permission_context` on every batch boundary so
/// tools always see the latest value.
/// - Plan-mode latches (`has_exited_plan_mode`, `needs_plan_mode_exit_attachment`).
/// - Permission-mode echo (`last_permission_mode`) for Reentry detection.
/// - Plan-mode entry timestamp (`plan_mode_entry_ms`) for verify-execution.
/// - Teammate approval handshake (`awaiting_plan_approval*`).
/// `PartialEq/Eq` is **not** derived: `PermissionRulesBySource` (used by
/// `stripped_dangerous_rules`) contains `PermissionRule` values which
/// aren't comparable. Tests compare fields individually.
#[derive(Debug, Clone, Default)]
pub struct ToolAppState {
    /// Live permission context base (mode, pre_plan, stripped, allow/deny/ask
    /// rules, additional_dirs, source_roots). The single live source of truth
    /// that `ToolContextFactory::build` snapshots each batch.
    /// `appState.toolPermissionContext`. See [`LiveToolPermissionState`].
    pub permissions: LiveToolPermissionState,

    // ── Plan-mode latches (one-shot signaling) ──
    /// Set by `ExitPlanModeTool` on success; read + cleared by the
    /// plan-mode reminder on the first following turn to emit the
    /// `Reentry` variant.
    pub has_exited_plan_mode: bool,

    /// One-shot: set by `ExitPlanModeTool` and by the reminder when it
    /// detects an unannounced mode transition. Cleared by the reminder
    /// after the exit-attachment is appended to history.
    pub needs_plan_mode_exit_attachment: bool,

    /// Outcome paired with `needs_plan_mode_exit_attachment` when the exit was
    /// produced by `ExitPlanModeTool`. `None` means the engine inferred an
    /// unannounced transition and cannot know whether there was a plan.
    pub pending_plan_mode_exit_outcome: Option<ExitPlanModeOutcome>,

    /// One-shot: set when leaving Auto mode (ExitPlanMode from a
    /// plan entered via Auto, or an unannounced Auto→non-Auto cycle
    /// detected by the reminder). Cleared by the reminder after the
    /// `## Exited Auto Mode` attachment is appended.
    /// `needsAutoModeExitAttachment` in `bootstrap/state.ts`.
    pub needs_auto_mode_exit_attachment: bool,

    /// `PermissionMode` from the prior turn. Reminder uses this to
    /// detect Plan ↔ non-Plan transitions; the driver uses it after a
    /// teammate plan approval to restore the leader's override.
    pub last_permission_mode: Option<PermissionMode>,

    /// UNIX-ms timestamp written by `EnterPlanModeTool`.
    pub plan_mode_entry_ms: Option<i64>,

    /// `true` while a leader is awaiting an approval reply from a teammate.
    /// Cleared by the reminder when the matching approval message arrives.
    pub awaiting_plan_approval: bool,

    /// Outstanding `plan_approval-<teammate>-<team>-<nonce>` correlation id
    /// for the current pending approval.
    pub awaiting_plan_approval_request_id: Option<String>,

    /// One-shot: set by `ExitPlanModeTool` when the user picked
    /// "clear context" in the multi-choice exit dialog. The engine
    /// consumes this at the next turn boundary by clearing history,
    /// appending [`pending_plan_implementation_message`], and resetting
    /// both fields.
    /// `initialMessage.clearContext = true` triggers REPL context
    /// clear when starting a new session.
    pub pending_clear_message_history: bool,

    /// User-role message appended after a plan-exit clear, so
    /// the fresh implementation turn still sees the approved plan.
    pub pending_plan_implementation_message: Option<String>,

    // ── Task / Todo snapshots ────────────────────────────────────────
    // Task tools emit `app_state_patch` closures that refresh these
    // fields after every mutation — the TUI reads them directly to
    // render the unified task panel. +
    // `AppState.todos[agentId]` mirrored across turns.
    /// Latest snapshot of the durable V2 plan-item list (visible
    /// entries only — `_internal` metadata items are filtered out by
    /// the tool before patching).
    pub plan_tasks: Vec<crate::TaskRecord>,

    /// V1 per-agent/per-session TodoWrite lists, keyed by
    /// `agent_id.unwrap_or(session_id)`. Empty until TodoWrite is used.
    pub todos_by_agent: std::collections::HashMap<String, Vec<crate::TodoRecord>>,

    /// Which panel the TUI should show expanded (task / teammates /
    /// none). Tools set this to [`ExpandedView::Tasks`] after create /
    /// update.
    pub expanded_view: crate::ExpandedView,

    /// When `true`, the TUI should surface a "spawn verification agent"
    /// banner above the input area. Set by `TaskUpdate` + `TodoWrite`
    /// when all items are completed, ≥3 items exist, and none match
    /// `/verif/i`. Cleared on acknowledgement or next TodoWrite cycle.
    pub verification_nudge_pending: bool,

    /// Monotonic generation for task-panel snapshots. Incremented under
    /// the state write lock by `ToolExecutor::apply_side_effects` for
    /// every applied patch and stamped into `TaskPanelChangedParams`.
    /// The leader's executor and the subagent/teammate bridges deliver
    /// on different channels with no global ordering, so consumers use
    /// this to drop snapshots that arrive after a newer one.
    pub panel_generation: i64,

    // ── Date-change latch ────────────────────────────────────────────
    /// Most recent local ISO date (`YYYY-MM-DD`) the engine emitted a
    /// `date_change` system-reminder for. The reminder subsystem fires
    /// when the current local date differs from this value and updates
    /// the latch atomically. `None` means no reminder has fired yet in
    /// this session — the first turn seeds the latch without emitting.
    /// `appState.lastEmittedDate` in `bootstrap/state.ts`,
    /// consumed by `getDateChangeAttachments`.
    pub last_emitted_date: Option<String>,

    // ── Plan verification ────────────────────────────────────────────
    /// Tracks a plan exit that has not yet been verified via
    /// `VerifyPlanExecution`. Set by `ExitPlanModeTool`; completed by the
    /// verification tool.
    /// `appState.pendingPlanVerification.{plan, verificationStarted,
    /// verificationCompleted}`.
    pub pending_plan_verification: Option<PendingPlanVerificationState>,

    // ── Worktree session state ───────────────────────────────────────
    /// Active foreground worktree entered by `EnterWorktree`.
    /// `ExitWorktree` reads this instead of trusting model-supplied paths,
    /// then clears it after returning to the original cwd. Background
    /// agent worktrees are tracked separately by the coordinator.
    pub active_worktree: Option<ActiveWorktreeState>,

    // ── Phase 2 delta-reminder announce state ────────────────────────
    /// Main-session compatibility mirror for the scoped announced-tool
    /// baseline. Use [`Self::last_announced_tools_for_scope`] and
    /// [`Self::set_last_announced_tools_for_scope`] for new reads/writes.
    pub last_announced_tools: HashSet<String>,

    /// Tool wire-names announced via the most recent `deferred_tools_delta`
    /// reminder, separated by visibility scope. The main session and each
    /// subagent can have different filtered tool sets; sharing one baseline
    /// makes a subagent's first turn look like the main session's tools were
    /// removed.
    pub last_announced_tools_by_scope: HashMap<String, HashSet<String>>,

    /// Wire-names of deferred tools the model has discovered via
    /// `ToolSearch` and that should now be exposed to the LLM with
    /// full schema (no longer deferred).
    /// `extractDiscoveredToolNames(messages)` in
    /// Walks message history each turn
    /// collecting `tool_name` from `tool_reference` blocks inside
    /// `tool_result.content`. coco-rs is provider-agnostic and cannot
    /// rely on Anthropic's server-side `tool_reference` expansion, so
    /// it persists the discovered set directly here. Tools that
    /// resolve via `ToolSearch` write through an `AppStatePatch`;
    /// `ToolRegistry::loaded_tools` consults this set to upgrade a
    /// `should_defer() == true` tool into the "loaded" pool for the
    /// next turn's tool-definitions build.
    /// **Invariant — additive only**: discovered names are NEVER
    /// removed from this set during a session. Once unlocked, a tool
    /// stays callable for the rest of the session and re-appears in
    /// every subsequent turn's `tools` array (the
    /// `tool_reference` block stays in history forever). Survives
    /// compaction automatically because the set lives on `ToolAppState`,
    /// not in messages — no `preCompactDiscoveredTools` carry-forward
    /// is required.
    /// `/clear` resets `ToolAppState` and therefore the set.
    /// **Cache cost**: on Anthropic + a model **without**
    /// `Capability::AnthropicToolReference`, each discovery grows
    /// the `tools` wire array by one entry and breaks the
    /// prompt-cache prefix once. After the model has discovered
    /// every tool it needs (typically a handful of early turns) the
    /// array is stable and the prefix stays warm.
    pub discovered_tool_names: HashSet<String>,

    /// Agent types announced via the most recent `agent_listing_delta`
    /// reminder. reconstructed from prior delta attachments.
    pub last_announced_agents: HashSet<String>,

    /// Per-server MCP instructions announced via the most recent
    /// `mcp_instructions_delta` reminder. Keyed by server name;
    /// value is the instruction text (hashable on content).
    /// reconstructed from prior delta attachments.
    pub last_announced_mcp_instructions: std::collections::HashMap<String, String>,

    /// Full normalized MCP-server announcement baseline per visibility scope.
    /// Counts and descriptions are part of the comparison so reconnects and
    /// metadata changes are announced; one agent can never suppress another
    /// agent's first scoped reminder.
    pub last_announced_mcp_servers_by_scope:
        HashMap<String, BTreeMap<String, McpServerAnnouncementState>>,

    // ── Agent progress summaries gate ──────────────────────────────
    /// Whether per-spawn periodic AgentSummary timers should run.
    /// Default `false`; the SDK control protocol's
    /// `agentProgressSummaries: true` flips this on.
    /// Coordinator mode forces it on regardless.
    /// Default-off matters for cost: a fully saturated coordinator
    /// (`MAX_IN_PROCESS_AGENTS = 16`) at the 30 s tick rate burns
    /// up to 32 side-query LLM calls per minute on summarization
    /// alone — opt-in semantics keep that off the user's hot path
    /// unless they explicitly request it.
    /// TUI users can flip this via `EnvKey::CocoAgentSummaryEnable`
    /// at session bootstrap; the env var maps onto this field
    /// without a separate signal path.
    pub agent_progress_summaries_enabled: bool,

    // ── Session presentation ────────────────────────────────────────
    /// Color of the prompt bar / standalone-agent badge for this session.
    /// `None` = the default theme color. Set by `/color <name>`, cleared
    /// by `/color default|reset|none|gray|grey`. Teammates inherit this
    /// from the leader's swarm assignment and ignore `/color`.
    pub agent_color: Option<AgentColorName>,

    // ── Stub-field wire-up (Phase 7) ───────────────────────────────
    /// Open permission overlays / coordinator-mailbox requests awaiting
    /// user response. Mutated lock-free by [`PendingPermissionGuard`]
    /// (`acquire`/`Drop` flips this counter via atomic ops). Read by
    /// `prompt_suggestion::build_suggestion_context` to gate
    /// `SuppressReason::PendingPermission`.
    /// `Arc<AtomicU32>` so the guard's `Drop` is fully synchronous —
    /// no `tokio::spawn`, no Tokio-runtime dependency, no deadlock
    /// against this struct's own `Arc<RwLock>` wrapper. Cloning the
    /// Arc is the canonical way to share the counter across the TUI
    /// overlay and coordinator mailbox without holding a write-lock.
    /// **Clone semantic.** `ToolAppState::clone` shares the same atomic
    /// (Arc semantic). Acceptable because clones are typically used for
    /// snapshotting where stale counter values are fine; callers that
    /// want a *fresh* counter construct via `Default`.
    pub pending_permission_count: Arc<AtomicU32>,

    /// In-flight MCP elicitation requests (form / URL). Same pattern
    /// as `pending_permission_count` — incremented when an
    /// `ElicitationRequest` is emitted, decremented on response /
    /// timeout / abort via [`ElicitationGuard`]. Read to gate
    /// `SuppressReason::ElicitationActive`.
    pub elicitation_pending_count: Arc<AtomicU32>,

    /// Per-provider rate-limit state, keyed by provider instance name
    /// (matches `services/inference::ProviderClientFingerprint::provider`,
    /// NOT the `ProviderApi` discriminator — two `OpenaiCompat`
    /// instances "groq" / "together" coexist independently).
    /// Mutated by the engine post-call (direct write under the
    /// app_state lock, same convention as `observers::ToolAppStateObserver`).
    /// Stale entries (`now > reset_at_ms`) are pruned at finalize_turn.
    /// Read by `prompt_suggestion::build_suggestion_context` to gate
    /// `SuppressReason::RateLimit` against `cache.provider`.
    pub rate_limits: BTreeMap<String, RateLimitEntry>,
}

impl ToolAppState {
    pub fn last_announced_tools_for_scope(&self, agent_id: Option<&str>) -> HashSet<String> {
        let key = announced_tools_scope_key(agent_id);
        self.last_announced_tools_by_scope
            .get(&key)
            .cloned()
            .unwrap_or_else(|| {
                if agent_id.is_none() {
                    self.last_announced_tools.clone()
                } else {
                    HashSet::new()
                }
            })
    }

    pub fn set_last_announced_tools_for_scope(
        &mut self,
        agent_id: Option<&str>,
        tools: HashSet<String>,
    ) {
        if agent_id.is_none() {
            self.last_announced_tools = tools.clone();
        }
        self.last_announced_tools_by_scope
            .insert(announced_tools_scope_key(agent_id), tools);
    }

    pub fn last_announced_mcp_servers_for_scope(
        &self,
        agent_id: Option<&str>,
    ) -> BTreeMap<String, McpServerAnnouncementState> {
        self.last_announced_mcp_servers_by_scope
            .get(&announced_tools_scope_key(agent_id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_last_announced_mcp_servers_for_scope(
        &mut self,
        agent_id: Option<&str>,
        servers: BTreeMap<String, McpServerAnnouncementState>,
    ) {
        self.last_announced_mcp_servers_by_scope
            .insert(announced_tools_scope_key(agent_id), servers);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerAnnouncementState {
    pub tool_count: usize,
    pub description: Option<String>,
}

fn announced_tools_scope_key(agent_id: Option<&str>) -> String {
    match agent_id {
        Some(id) => format!("agent:{id}"),
        None => "main".to_string(),
    }
}

/// Foreground worktree state stored on [`ToolAppState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWorktreeState {
    pub original_cwd: std::path::PathBuf,
    pub worktree_path: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// SHA of the **resolved default branch** (e.g. `origin/main`) the worktree
    /// was created from — NOT the repo's current HEAD. Lets `ExitWorktree` report
    /// `discardedCommits` = commits on the worktree branch ahead of that base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_head_commit: Option<String>,
}

// ────────────────────────────────────────────────────────────────
// RAII counter guards (Phase 7 stub-field wire-up)
// ────────────────────────────────────────────────────────────────

/// Increment-on-acquire / decrement-on-Drop guard around an
/// `Arc<AtomicU32>` counter. Used to track open permission overlays
/// and pending coordinator-mailbox requests so the prompt-suggestion
/// fork can suppress when one of those flows is awaiting user input.
/// **Lock-free.** `Drop` performs a single relaxed atomic decrement —
/// no `tokio::spawn`, no Tokio-runtime dependency, no deadlock risk.
/// Safe to drop from a panicked task or non-Tokio thread.
/// **Why `Ordering::Relaxed`.** The counter is self-contained: readers
/// only need eventual visibility for the boolean "is anything pending?"
/// check, not happens-before with other state.
#[derive(Debug)]
pub struct PendingPermissionGuard {
    counter: Arc<AtomicU32>,
}

impl PendingPermissionGuard {
    pub fn acquire(counter: Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for PendingPermissionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Same shape as [`PendingPermissionGuard`], pinned to MCP elicitation
/// requests. Held inside the pending-elicitations entry so timeout /
/// abort / response all decrement the counter exactly once via
/// `Drop`.
#[derive(Debug)]
pub struct ElicitationGuard {
    counter: Arc<AtomicU32>,
}

impl ElicitationGuard {
    pub fn acquire(counter: Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for ElicitationGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

// ────────────────────────────────────────────────────────────────
// App-state handle + queued-patch types (tool-facing API surface)
// ────────────────────────────────────────────────────────────────
// `ToolUseContext.app_state` holds an `AppStateReadHandle` — a wrapper
// around `Arc<RwLock<ToolAppState>>`. Most tool mutations should still
// flow through queued patches, but tools that must make state durable
// before another process/session side effect may take a direct write lock.
// Mutations flow through `ToolResult::app_state_patch`: a boxed
// `FnOnce(&mut ToolAppState)` that the executor applies post-execute
// (serial) or post-batch (concurrent) under a single write lock.
// This exactly:
// tools return a `(ctx) => newCtx` modifier; the orchestrator queues
// them per tool_use_id and applies after the concurrent batch. No
// tool can observe another tool's mutation mid-batch, and no tool
// can observe another queued mutation mid-batch.

/// Handle to the shared [`ToolAppState`]. Tools receive
/// this on [`crate::ToolUseContext::app_state`] and can query live
/// state via [`AppStateReadHandle::read`]. Ordinary mutations return
/// an [`AppStatePatch`] through [`crate::ToolResult::app_state_patch`]
/// instead.
/// `appState.toolPermissionContext` is visible via
/// `context.getAppState()`, but writes go through
/// `context.setAppState(...)` which the orchestrator funnels into
/// `queuedContextModifiers` for post-batch apply. Rust keeps that as
/// the default path while still exposing a write lock for tools whose
/// state update must precede another side effect.
/// Non-tool callers (engine, reminder, TUI / SDK mode handlers)
/// that architecturally *are* authorized to mutate hold the
/// underlying `Arc<RwLock<ToolAppState>>` directly; they never
/// route through this handle.
#[derive(Debug, Clone)]
pub struct AppStateReadHandle {
    inner: Arc<RwLock<ToolAppState>>,
}

impl AppStateReadHandle {
    /// Wrap an existing shared state Arc.
    pub fn new(inner: Arc<RwLock<ToolAppState>>) -> Self {
        Self { inner }
    }

    /// Acquire a read lock. Tools use this to inspect live state
    /// (e.g. `ctx.app_state.as_ref()?.read().await.permission_mode`).
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, ToolAppState> {
        self.inner.read().await
    }

    /// Acquire a write lock for tools that must update app state before
    /// another side effect, such as changing the session cwd.
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, ToolAppState> {
        self.inner.write().await
    }
}

impl From<Arc<RwLock<ToolAppState>>> for AppStateReadHandle {
    fn from(arc: Arc<RwLock<ToolAppState>>) -> Self {
        Self::new(arc)
    }
}

/// A mutation of the shared [`ToolAppState`], queued by a tool via
/// [`crate::ToolResult::app_state_patch`] and applied by the
/// executor after `execute` returns.
/// `update.newContext: (ctx) => ctx` in
/// `orchestration.ts`. Per-tool, ordered by submission (= TS
/// `Object.entries(queuedContextModifiers)` iteration order), applied
/// under a single write lock so intermediate states are never
/// observable.
pub type AppStatePatch = Box<dyn FnOnce(&mut ToolAppState) + Send + Sync + 'static>;

#[cfg(test)]
#[path = "app_state.test.rs"]
mod tests;
