# Multi-Session App Server Plan for coco-rs

Status: design, v6 (post-overall-review hardening). Supersedes the
direction in [`concurrent-app-server-plan.md`](concurrent-app-server-plan.md)
by replacing the codex-rs `ThreadId`/thread-store model with coco-rs
session-root routing.

v6 closes 9 findings from an end-to-end pass over v5: `TurnId` typed
newtype (was used in §7.2/§14 but undefined in §2), `task_set` field
added to the `SessionRuntimeHandle` clause (was referenced 5× in prose
but missing from the field list), three-kind subagent taxonomy
(`LocalAgent` fg/bg + `InProcessTeammate`; bg + teammate register into
`task_set`), `coco-coordinator` session-scoped state integration into
`SessionRuntimeHandle` (PR #3 extracted 21 modules but v5 mentioned
the crate once), `Loading × archive` race semantics (archive awaits
in-flight load), `cwd_override.is_dir()` check (was just
`.exists()`, accepted regular files), `archived: true` filter
includes `Archiving` entries, `ClientError` enum variants enumerated,
teammate-as-session marked as wire-forward-compatible v2 direction.
Decision log entries #47–#54 are new.

v5 closed 11 substantive findings from a v4 cross-review: replace
algorithm atomicity (two-phase commit; old untouched until new is
committed), archive-vs-resume semantics (archive = runtime close, not
tombstone; resume always reloads on-disk), `Arc::strong_count` →
explicit `JoinSet` drain, hot-reload single-rule model (snapshot at
session-start), multi-client replace routing, Hub subprotocol v2 for
per-session seq, stale-cwd `cwd_override` + `CwdNotFound`,
`McpServerName` typed newtype, TUI `/resume` archives current first.
Decision log entries #15/#16/#27/#31 are revised; #41–#46 are new.

Companions: see [`event-hub/spec.md`](event-hub/spec.md),
[`crate-coco-types.md`](crate-coco-types.md),
[`crate-coco-app.md`](crate-coco-app.md),
[`crate-coco-query.md`](crate-coco-query.md), and
[`subagent-refactor-plan.md`](subagent-refactor-plan.md).

## 0. Decision

`coco-rs` runs one server process with many concurrent root sessions and
never imports codex-rs physical-thread semantics.

The server replaces the current single-active-SDK-session slot with a
registry of root-session runtimes keyed by `SessionId`:

    sessions: RwLock<HashMap<SessionId, SessionEntry>>

The public protocol surface is `session/*` and `turn/*`. **Every
session-scoped request carries a mandatory `session_id`** — no implicit
defaults, no per-connection active-session state. Notifications emitted
for session-scoped work carry `session_id` so clients can demultiplex.
No `ThreadId` exists on the wire, on disk, in transcript rows, in Hub
envelopes, or in `coco-types`.

Connection identity is deliberately not public protocol state. The server
owns an internal opaque `ConnectionKey` for outbound routing, subscription
membership, disconnect fan-out, and pending-RPC drainage. It must never
appear on the wire, on disk, in Hub envelopes, or in `coco-types`.

One client binds to one session, but multiple clients MAY rejoin the same
session via `session/resume` (idempotent) and share the event stream.

Per-connection convenience (1-client-1-session binding, hidden session
lifecycle) is built at the SDK library layer, not at the protocol layer.

## 1. Current State

`app/cli/src/sdk_server/handlers/mod.rs:242` owns:

    session: RwLock<Option<SessionHandle>>

This is intentionally single-session. `app/cli/src/session_runtime.rs:398-727`
mixes ~40 process-scoped and session-scoped fields in one `SessionRuntime`,
with `session_id: Arc<RwLock<String>>` (line 474) repointed by
`clear_conversation()` on /clear/resume paths.

MCP is process-global: `app/cli/src/main.rs:360-365` builds one
`Arc<tokio::sync::Mutex<McpConnectionManager>>` and shares it via
`server.with_mcp_manager(...)` on line 439. Per-session MCP isolation
requires real surgery (see §16).

Persistence already uses the correct identity model:

    <project>/<session_id>.jsonl
    <project>/<session_id>/subagents/agent-<agent_id>.jsonl
    <project>/<session_id>/subagents/agent-<agent_id>.meta.json

(`utils/coco-paths/src/project_paths.rs:96-119`). Preserve this layout.

`SessionId` and `AgentId` typed wrappers already exist in
`common/types/src/id.rs:6-23, 49-105` as `pub struct(pub String)` thin
brands. Migration hardens them into validated path-safe newtypes with
private fields and fallible serde before exposing them on app-server-facing
structs.

## 2. Identity Model

`coco-types` owns typed string identities used across crate boundaries:

- `SessionId`: root-session transcript identity and public SDK handle
- `AgentId`: subagent identity under a root session
- `TaskId`: existing task identity, unchanged
- `TurnId`: per-turn server-generated UUID v4. Surfaced in `turn/start`
  response and on every `turn/*` notification so clients correlate
  streaming deltas, disambiguate batched `turn/interrupted` emissions
  when interrupt drains the pending queue, and key per-turn telemetry.
  NOT a path component (no `.`/`/` validation needed); thin newtype +
  serde-as-string. `pub struct TurnId(String)` with `as_str()` accessor;
  `TryFrom<&str>` validates UUID v4 textual shape only.

`SessionId` and `AgentId` are file-path components, so branding alone is
insufficient. They validate at construction/deserialization time:

- `SessionId` canonical format is server-generated UUID v4 text, with no
  `session-` prefix
- `AgentId` keeps the existing generated shape
  `a[optional-label-][16-hex-chars]`; labels are restricted to ASCII
  lowercase letters, digits, `_`, and `-`, and must not contain path
  separators or dot segments
- Both types reject empty strings, `/`, `\`, `.`, `..`, and any platform
  path separator
- Inner field becomes private (`pub struct SessionId(String)`); access via
  `as_str()` / `into_inner()` only
- `From<&str>` infallible impl is removed; only `TryFrom<&str>` available
- Path-building APIs accept `&SessionId` / `&AgentId`, not `&str`, once
  migration reaches storage callsites

No `RuntimeKey`-style internal routing enum. Server-side maps key by
`SessionId` directly; subagent identity surfaces only at the task-runtime
layer keyed by `(SessionId, AgentId)`. Logs use
`format!("session={session_id} agent={agent_id:?}")`.

### 2.1 Root Sessions

A root session is the public conversation unit:

- one root transcript file: `<session_id>.jsonl`
- one app-server routing entry keyed by `SessionId`
- one active-turn slot + FIFO pending queue
- one session-scoped command queue
- one session-scoped MCP manager (subject to `McpScope`, §16)
- one event stream tagged with `session_id`; potentially multiple
  subscriber connections (see §9)

`session/start` allocates a new `SessionId` (server-generated UUID).
`session/resume` opens an existing `SessionId` (idempotent across "rejoin
running" and "load from disk", §7.1).
`session/archive` cancels and removes one root session runtime.
`session/replace` atomically swaps an old session for a new one.

### 2.2 Subagents

Subagents are not app-server root sessions. They remain owned by
`coco-coordinator` and the AgentTool task runtime, persisted at:

    <session_id>/subagents/agent-<agent_id>.jsonl
    <session_id>/subagents/agent-<agent_id>.meta.json

Subagent events may include `agent_id` as optional debug context but
always scope under `session_id`. A subagent never introduces a `ThreadId`
or a separate root transcript identity.

**Three task-kind lifecycles** (`coco_types::TaskType`, see
`docs/coco-rs/agentteam-architecture.md`):

| Kind | Lifetime | Parent-turn binding | task_set membership |
|---|---|---|---|
| `LocalAgent` foreground | Within parent turn | Cancelled by parent turn's `CancellationToken` (descends from `session_token`) | NO — completion bound to the turn |
| `LocalAgent` background (`run_in_background=true` or `AgentDefinition.background`) | Outlives parent turn; ends on completion, kill, or session archive | Independent `CancellationToken` descended from `session_token` | YES — joined via `task_set` |
| `InProcessTeammate` (long-lived teammate with named identity + mailbox + pane) | Spans many leader turns until `ShutdownRequest` or archive | Independent `CancellationToken` descended from `session_token` | YES — joined via `task_set`; `runner_loop` task per teammate |

`RemoteAgent` (CCR / teleport) is explicitly skipped in coco-rs
(agentteam doc §3 non-goals).

`SendMessageTool::resume_agent` (TS-aligned auto-resume of a
terminated background agent) runs **inside the parent turn**. It
spawns a fresh subagent with `SpawnMode::Resume`; it does NOT enqueue
a new session turn and does NOT participate in the session FIFO.

### 2.3 Subagent Inheritance

Inherited from parent root session: process-scoped registries, parent
permission tier, parent MCP server set (subject to scope, §16), shared
command/skill/agent catalogs, `Arc<Features>` (resolved once at process
boot per `Features::with_defaults() → settings → env → runtime
overrides` per root `CLAUDE.md`), parent transcript path scope,
`config_snapshot`.

Fresh per subagent: own `AgentId`, own transcript file, own
`ToolUseContext`, own active-turn cancellation handle, own usage/budget
tracker.

Subagents must never widen `Features` beyond parent; AgentTool enforces.
The parent's Features set is the process-resolved `Arc<Features>` —
there is no per-session narrowing layer in v1, so the subagent parent
set equals the process default.

## 3. Non-Goals

Do not add in v1:

- `app/thread` or `coco-thread`
- public or disk-level `ThreadId`
- codex-rs thread-store semantics
- `<thread_id>.jsonl` files
- Hub envelopes with `thread_id`
- a large protocol crate extraction
- `session/reset` RPC with carry-forward or history-retarget semantics
- `session/switch` RPC
- per-connection `active_session` defaults
- public or wire-level `ConnectionId`
- `carry_forward` on `/clear`
- multi-session TUI UI (single-active-session only; multi-window
  deferred)
- SDK `with_session(id)` cross-session escape hatch
- SDK `client.reconnect()` in-place API
- SDK auto-reconnect / auto-resume
- server-side session auto-GC, idle timeout, or LRU eviction
- client-proposed `session_id` values
- per-session config overlays (model factory, plugins, hooks, settings
  remain process-global)
- per-session OAuth identity for MCP (PerSession scope shares process
  OAuth credentials, §16)
- **cross-session file-system isolation** (concurrent edits to the same
  file are user responsibility; no advisory locks)
- **per-session `Feature` overrides** (Features resolve once at process
  boot per `Features::with_defaults() → settings → env → runtime
  overrides`; no session-level narrowing or widening API in v1)
- **independent `worktree` parameter on `session/start`** (worktree is
  governed entirely by process-level `Feature::Worktree` via
  `settings.json:worktree.enabled`, `COCO_FEATURE_WORKTREE` env, or
  CLI flag)
- SDK overrides of MCP `symlink_directories` / advanced worktree
  sub-config (process settings only in v1)

Backward compatibility with the current single-session SDK flow is not a
requirement.

### 3.1 Future direction (out of v1 scope)

**Teammate-as-session promotion.** `coco-coordinator`'s
`InProcessTeammate` task kind (long-lived teammate with named identity,
mailbox, pane backend) is structurally close to a root session — it
already carries a conversation, transcript, cwd, model selection, and
permission state. A future v2 could promote it: each teammate gets
its own `SessionId`, transcripts move from
`<leader_session_id>/subagents/agent-<agent_id>.jsonl` to
`<teammate_session_id>.jsonl` (top-level layout), mailbox becomes a
cross-session communication primitive keyed by a new `TeamId`
identity, and `TeamCreateTool` dispatches to `session/start` with
subagent metadata instead of `SwarmAgentHandle::spawn_subagent`.

The v1 wire surface is forward-compatible with this evolution:

- `session/start` parameter extensibility is non-breaking
- Per-session `session_seq` already supports per-teammate event
  ordering at the Hub
- `Client::connect_with_session(opts, teammate_session_id)` already
  binds an external client to a single session (pane backends could
  reuse it instead of spawning standalone `coco` processes)

v2 is a **disk-layout migration + new `TeamId` identity**, not a
wire-protocol break. v1 deliberately keeps teammates owned by
`coco-coordinator` under `<leader_session_id>/subagents/` to avoid
the layout change cost when teammate-as-session isn't yet justified
by a concrete consumer.

## 4. Crate Boundaries

### 4.1 New Crates

`app/runtime` → `coco-app-runtime` (Tier 3: snafu + coco-error)
- `SessionRuntimeFactory`
- `SessionRuntimeHandle`
- process/session split extracted from
  `app/cli/src/session_runtime.rs`
- per-turn `QueryEngine` construction
- transcript, message history, command queue, app state, active turn,
  session-scoped MCP wiring

`app/server-transport` → `coco-app-server-transport` (Tier 2: thiserror)
- stdio NDJSON transport
- UDS WebSocket transport
- TCP WebSocket transport
- JSON-RPC framing

Pure I/O. No coco domain state. It yields accepted connection streams to
`coco-app-server`; the server assigns the private `ConnectionKey`.

`app/server` → `coco-app-server` (Tier 3: snafu + coco-error)
- `MessageProcessor`
- internal `ConnectionKey` registry (not in `coco-types`)
- session registry map (§6)
- per-session serialization queues (§8)
- outbound routing (forward + reverse fan-out maps, §9)
- subscription management
- server lifecycle
- transport-close graceful drain (outbound flush, subscription
  cleanup, RPC fan-out cancellation). The `Disconnected` notification
  itself is **SDK-synthesized client-side** (§14); server-side never
  emits it (the outbound queue is often already full when transport
  drops — server-side delivery would race against its own shutdown).
- slow-consumer disconnect (§9)

`app/server-client` → `coco-app-server-client` (**Tier 2: thiserror**;
no `coco-error` dependency in public API)
- in-process `LocalTransport` (typed direct, no serde) for TUI
- UDS/WS `RemoteTransport` (JSON-RPC) for external SDK
- typed helpers over `ClientRequest`, `ServerNotification`,
  `JsonRpcMessage`
- 1-client-1-session SDK binding (§14)

### 4.2 Deferred Crate

Do not add `app/server-protocol` in v1. Keep `ClientRequest`,
`ServerNotification`, and `JsonRpcMessage` in `coco-types`. Reconsider
after the app-server split is stable.

## 5. Runtime Split

Extract `SessionRuntime` (`app/cli/src/session_runtime.rs:398-727`,
~40 fields) into two layers. A separate `runtime-field-map.md` appendix
will enumerate every current field as ProcessRuntime / SessionRuntimeHandle /
"delete".

### 5.1 Process Runtime

Owns:

- resolved configuration snapshot (`Arc<RuntimeConfig>`) and reload hooks
- model/client factory and client cache
- tool, command, skill, hook, plugin, MCP-shared registries
- long-lived shared services
- outbound server event sink factory
- process-level feature gates and runtime overrides

Does not own message history, active turn state, session memory, or
transcript state.

**Single rule — snapshot at session-start, frozen for session
lifetime.** Every piece of process-level state a session reads —
`RuntimeConfig`, tool/command/hook/plugin/skill registries, MCP-shared
catalog, model-client cache — lives behind `arc-swap::ArcSwap` (or
`tokio::sync::watch` per the existing `RuntimePublisher` pattern at
`common/config/src/runtime.rs:639`). Process-level mutators
(`settings/update`, `plugin/reload`, `mcp/setServers`) construct fresh
state and call `ArcSwap::store` / `watch::Sender::send` — no
mutation-in-place, no write lock held across `.await`.

Each `session/start` clones the current `Arc<T>` for **every** such
piece into the new `SessionRuntimeHandle` (fields like
`config_snapshot`, `tool_registry`, `plugin_registry`, …). For the
session's lifetime, code under that session reads ONLY the cloned
`Arc<T>`s; it never re-reads from the process-level slots. Mid-session
`settings/update` / `plugin/reload` therefore has **zero** effect on
running sessions — they observe the new snapshot only on the next
`session/start` (or `session/replace`, which is effectively
"close + start").

This is one rule, no exceptions. It eliminates the v4-era
contradiction between "ArcSwap registries — next lookup sees new
version" and "RuntimeConfig snapshot stable for session lifetime":
both classes of state now follow the snapshot-at-start model. Hot
reload is still useful — future sessions pick up new state — but it
no longer threads through any in-flight session's runtime view.

**API note.** `std::sync::Arc` does NOT expose `swap`; the atomic swap
APIs are `arc_swap::ArcSwap::store` / `load_full` and
`tokio::sync::watch::Sender::send`. Earlier drafts of this doc said
"`Arc::swap`" — that was shorthand, not a real Rust API.

### 5.2 Session Runtime Handle

`SessionRuntimeHandle` owns one root session. Stable (Arc'd, not
swapped in the registry); interior mutability lives behind locks and
channels. Required `Send + Sync`.

Fields:

- `session_id: SessionId`
- `cwd: Arc<RwLock<PathBuf>>` (session-scoped; written by BashTool
  post-processor via `ToolUseContext`)
- `config_snapshot: Arc<RuntimeConfig>` (snapshot at start; see §5.1)
- `features: Arc<Features>` (clone of process-resolved Features;
  passed to subagents as their parent)
- `query_engine_config: QueryEngineConfig`
- `message_history`
- `command_queue`
- `app_state`
- `attachment_inbox`
- `active_turn: Mutex<Option<RunningTurn>>`
- `pending_turns: Mutex<VecDeque<QueuedTurn>>`
- `session_token: CancellationToken` (parent of all turn tokens)
- `task_set: Mutex<JoinSet<()>>` — registry for long-lived
  session-spawned tasks. Membership: bg `LocalAgent` spawns,
  `InProcessTeammate` `runner_loop`s, PerSession MCP supervisor,
  mailbox watchers, transcript-flush writer, periodic-summary timer,
  any other `tokio::spawn` made under this session. Archive drains via
  `task_set.shutdown().await` (see §6 archive cascade, step 6).
- `transcript_store` handle for `<session_id>.jsonl`
- `session_usage` tracker
- `session_memory`
- `session_mcp` manager (Shared reference or PerSession owned; see §16)
- `session_permission_rules: RwLock<Vec<PermissionRule>>` (session-tier
  permission rules added via permission-mode updates; cleared on replace)
- `worktree_state: RwLock<Option<PersistedWorktreeSession>>` (set by
  `EnterWorktree` tool, cleared by `ExitWorktree`, restored from
  transcript metadata on resume)
- `session_span: tracing::Span` (root OTel span for this session)
- `event_sink` tagged with `session_id`

Tools never resolve a session by registry lookup. Session-scoped Arcs
(notably `cwd`) are reached through `ToolUseContext`; the BashTool
post-processor writes into `ctx.cwd_arc()`.

Background tasks (transcript writer, compaction, MCP per-session
manager, OTel exporter) hold `Weak<SessionRuntimeHandle>` — not `Arc`
— to avoid keeping archived sessions alive past `archive`. Strong
refs exist only inside the registry and synchronously-borrowed handler
paths.

Every long-lived task spawned **under** a session is registered into
the session's `task_set: JoinSet<()>` (or equivalent). Archive's
shutdown signal is `session_token.cancel()`; archive then
`task_set.shutdown().await`s for all registered tasks to join.
Lifecycle is gated by **explicit task join**, NOT by
`Arc::strong_count` polling — which would be paradoxical (archive
itself needs a strong handle to drive the cascade) and brittle in the
face of any un-audited `Arc::clone` anywhere in the codebase.

Session-scoped code must not read or mutate process-global cwd. During the
split, every `std::env::current_dir()` / `std::env::set_current_dir()` use
under prompt assembly, hooks, reminders, compaction, worktree cleanup, and
tool defaults is audited and either moved to process bootstrap or replaced
with the session cwd carried by `SessionRuntimeHandle` / `ToolUseContext`.
Audit scope spans ~12 crates: `coco-context`, `coco-system-reminder`,
`coco-compact`, `coco-hooks`, `coco-tools/file`, `coco-shell`,
`coco-commands`, `coco-skills`, `coco-tasks`, `coco-memory`,
`coco-retrieval`, `coco-lsp`.

`QueryEngine` may still be built per turn; the per-turn engine must be
built from a stable per-session handle, never from global mutable state.

## 6. Server Registry

`SessionRegistry` owned by `coco-app-server`:

```rust
struct SessionRegistry {
    sessions: RwLock<HashMap<SessionId, SessionEntry>>,
    max_sessions: usize,
}

enum SessionEntry {
    /// Single-flight resume in progress; concurrent callers await
    /// the same future.
    Loading(SharedLoadFuture),

    /// Live session, serving RPCs.
    Ready(Arc<SessionRuntimeHandle>),

    /// Archive cascade running; handle still serves `get`, `resume`,
    /// and in-flight RPCs until the cascade completes and the entry
    /// is removed (§6 archive cascade order, §6 replace Stage 2-3).
    Archiving(Arc<SessionRuntimeHandle>, SharedArchiveFuture),
}

type SharedLoadFuture =
    futures::future::Shared<BoxFuture<'static,
        Result<Arc<SessionRuntimeHandle>, ResumeError>>>;
type SharedArchiveFuture =
    futures::future::Shared<BoxFuture<'static, ()>>;

#[derive(thiserror::Error, Debug, Clone)]
enum ResumeError {
    #[error("session not found: {0}")]
    NotFound(SessionId),
    #[error("max_sessions limit reached")]
    ResourceExhausted,
    #[error("session id format invalid: {0}")]
    Invalid(String),
    #[error("transcript load failed ({kind:?}): {message}")]
    LoadFailed { kind: TranscriptLoadKind, message: String },
    #[error("recorded cwd no longer exists: {recorded_cwd}")]
    CwdNotFound { recorded_cwd: PathBuf },
}

#[derive(Debug, Clone, Copy)]
enum TranscriptLoadKind { Io, ParseError, MissingHeader, Truncated }
```

`ResumeError` MUST be `Clone` to satisfy
`futures::future::Shared`'s result-type bound — concurrent same-id
callers receive the same error. Therefore `LoadFailed` carries
`(kind, message)` instead of `#[from] std::io::Error` (which is **not**
`Clone`); conversion happens at the load-site:

```rust
file.read_lines().await.map_err(|e| ResumeError::LoadFailed {
    kind: TranscriptLoadKind::Io,
    message: e.to_string(),
})
```

The whole error surface implements `coco_error::StackError +
ErrorExt + StatusCode` to fit Tier 3 — same pattern as
`coco-session::error::SessionError` (`app/session/src/error.rs:14`).
`thiserror` is the derive crate; the Tier 3 ergonomics come from the
hand-written `StackError`/`ErrorExt` impls. The root `CLAUDE.md`
phrase "snafu + coco-error" is shorthand for "implements
`coco_error::ErrorExt`" — `thiserror` is acceptable when those impls
are present.

**Single-flight rule:** when `session/resume(id)` finds neither `Ready`
nor an existing `Loading` entry, the resolver:

1. Acquires write lock on `sessions`
2. Inserts `SessionEntry::Loading(shared_future)` placeholder
3. Releases the lock
4. Concurrent callers seeing `Loading(future)` await the same future
5. On success, the resolver re-acquires the write lock, swaps
   `Loading → Ready(handle)`, releases
6. On failure, the resolver removes the entry; next caller retries
   from scratch

`Loading`, `Ready`, and `Archiving` all count toward `max_sessions`.
The single exception is `session/replace`, which bypasses the limit by
+1 for its own duration (see replace accounting below).

**Operations:**

```rust
async fn create(params: SessionStartParams)
    -> Result<(SessionId, Arc<SessionRuntimeHandle>), CreateError>;

async fn resume(id: SessionId, params: SessionResumeParams)
    -> Result<Arc<SessionRuntimeHandle>, ResumeError>;
    // Idempotent rejoin-or-load with single-flight semantics

async fn replace(old: SessionId, params: SessionStartParams)
    -> Result<(SessionId, Arc<SessionRuntimeHandle>), ReplaceError>;
    // Atomic — see "Replace algorithm" below

async fn archive(id: SessionId) -> Result<(), ArchiveError>;
    // See "Archive cascade" below

fn get(id: &SessionId) -> Option<Arc<SessionRuntimeHandle>>;
fn list_active() -> Vec<SessionId>;
fn contains(id: &SessionId) -> bool;
```

**`SessionEntry` state machine.** Slot states and legal transitions:

```
                        single-flight resume
                  ┌──────────────────────────┐
                  ▼                          │
session/start → Ready ──── archive ────► Archiving ──► (removed)
                  ▲                          │
                  │                          │ resume(id) during
session/resume(loadable on disk)             │ Archiving returns
   creates Loading ──success──┘              ▼ handle_old until
   creates Loading ──failure──► (removed)    cascade finishes
```

Invariant: a `SessionId` occupies at most one `SessionEntry` slot at
any instant. `Ready`, `Loading`, and `Archiving` all count toward
`max_sessions`. The single exception is `session/replace`, which
deliberately occupies +1 transient slot for the lifetime of the
replace operation (Stage 1 → Stage 3); see "Max-sessions accounting
during replace" below.

**Archive cascade order:**

1. Set `session_token` → cancelled (cascades to all turn tokens)
2. Drop pending queue; emit `turn/interrupted` for each queued turn
3. Wait `active_turn` to reach drain point (next await boundary)
4. Send SIGTERM to PerSession MCP children; grace period 5s; SIGKILL
5. Flush transcript writer (await pending writes complete)
6. `task_set.shutdown().await` — explicit join of all session-spawned
   background tasks (§5.2). No `Arc::strong_count` polling.
7. Remove entry from registry; emit `session/ended`

Transcript file preserved (not deleted). Archive is a **runtime
close**, not a tombstone. The archived `<session_id>.jsonl` remains
on disk and is re-openable via `session/resume(session_id)` — see
§7.1 archive semantics and §7.1 resume disk-load rule.

**Archive on a `Loading` entry.** Single-flight rule means a resume
race can leave `SessionEntry::Loading(future)` in place when `archive`
arrives. Behavior:

1. If `archive(id)` finds `Loading(future)` — await the load future to
   resolve.
2. Load failure → entry already removed by single-flight failure
   path; archive returns `Ok(())` (nothing to drain).
3. Load success → entry was just swapped to `Ready(handle)`; archive
   re-acquires the registry write lock, transitions `Ready → Archiving`,
   then runs the cascade above on `handle`.

The await happens outside any registry lock. This deliberately
prefers "finish what was in flight, then close it cleanly" over
"abort the load" because the load's IO side-effects (reading
`<session_id>.jsonl` into memory) are already happening regardless.

**Replace algorithm (two-phase commit; old untouched until new is
committed).** v5 inverts v4's "construct new in parallel with cascade
on old" — that was non-atomic (a Stage-1 failure left old's MCP
killed, transcript flushed, queue drained, with no rollback path). v5
keeps old fully operational throughout new's construction; failure
just drops new's Loading entry.

```
Stage 1 — Build new (caller awaits):
1. Under registry write lock:
   a. Verify old is Ready(handle_old); else fail with
      `ReplaceError::OldNotReady`.
   b. Generate new_id (server UUID).
   c. Reserve new_id slot as Loading(future_for_new_handle).
      max_sessions check at this point: replace bypasses the limit
      by +1 (see accounting note below).
   d. Release lock. Old's slot is unchanged — still Ready, still
      serving all RPCs.
2. Background task constructs new handle. Old continues running its
   active turn, accepting turn/start, replying to mcp/* etc.
3. On construction failure:
   a. Under registry write lock: remove new_id Loading entry.
   b. Return ReplaceError::ConstructFailed { cause }. Old is fully
      intact — no MCP killed, no transcript flush, no queue drain.

Stage 2 — Atomic commit (single write-lock section):
4. New handle constructed. Under registry write lock:
   a. Swap new_id: Loading → Ready(new_handle).
   b. Re-mark old: Ready(handle_old) → Archiving(handle_old, archive_future).
      get(old_id) still returns handle_old; concurrent
      session/resume(old_id) attaches to the archive_future and
      receives the handle until cascade finishes.
   c. Update caller's ConnectionKey routing atomically: K→old becomes
      K→new (so the requesting client never observes a routing gap).
   d. Release lock.
5. Emit session/started(new_id) to caller.
6. Emit session/replaced { old: old_id, new: new_id } to all
   ConnectionKeys still in old's reverse fan-out — these are the
   OTHER multi-client subscribers; the caller's K has already moved.
   Each peer decides whether to migrate (§9 multi-client replace
   routing).

Stage 3 — Background archive:
7. Spawn archive cascade on handle_old per "Archive cascade order"
   above. Independent of caller.
8. On cascade completion: under registry write lock, remove old's
   Archiving entry.
9. Emit session/ended(old_id).
```

**Rollback matrix:**

| Failure point      | New state              | Old state          |
|--------------------|------------------------|--------------------|
| Stage 1 construct  | Loading entry removed  | Ready (fully OK)   |
| Stage 2 commit     | unreachable (single lock section, no .await mid-commit) | — |
| Stage 3 cascade    | Ready (committed)      | Archiving; cascade is idempotent — re-runnable on restart from `Archiving` entry persisted in registry, or simply left to drain |

**Max-sessions accounting during replace.** During Stage 1: old=Ready
+ new=Loading occupy 2 slots transiently. During Stage 2 commit
window: old=Archiving + new=Ready still 2 slots. After Stage 3: 1
slot (new only). Replace **bypasses `max_sessions` by +1 for its own
duration** — it is a swap, not a new capacity grant. A concurrent
`session/start` racing during replace still sees the full
`max_sessions` limit and may receive `ResourceExhausted`; only the
replace operation itself enjoys the +1 transient.

**Why Stage 2 cannot fail.** The commit is a single write-lock
section with no `.await`: it performs in-memory map updates only. If
the panic-recovery story ever changes (e.g., to persist `Archiving`
to disk inside the lock), Stage 2 must be split — flag this in
review.

## 7. Request Handling

All session-scoped requests carry mandatory `session_id`.

### 7.1 Session Requests

`session/start` — params:

```rust
struct SessionStartParams {
    cwd: PathBuf,                              // required
    model_role: Option<ModelRole>,             // default Main
    permission_mode: Option<PermissionMode>,
    thinking: Option<ThinkingMode>,
    initial_attachments: Option<Vec<Attachment>>,
}
```

- Server generates `SessionId` (UUID v4); clients cannot propose
- Snapshots `Arc<RuntimeConfig>` at start (see §5.1)
- Clones process-resolved `Arc<Features>` into the session handle; no
  per-session Feature overrides in v1 (see §3 non-goals)
- Enforces `max_sessions`; over-limit returns `ResourceExhausted`
- Inserts into registry; emits `session/started` with `session_id`
- Returns `SessionStartResult { session_id }`

`session/resume` — params:

```rust
struct SessionResumeParams {
    session_id: SessionId,

    /// Override the cwd recorded in the transcript. Required when the
    /// recorded cwd no longer exists; optional otherwise (the
    /// recorded cwd is used). Server NEVER falls back to
    /// `std::env::current_dir()`.
    cwd_override: Option<PathBuf>,
    // No feature_overrides on resume; Features are process-resolved.
}
```

- **Idempotent open** (parallels codex-rs `thread/resume` at
  `app-server-protocol/src/protocol/v2/thread.rs:320-328`):
  - Registry has `Ready(handle)` → return same handle
  - Registry has `Loading(future)` → await same future
  - Registry has `Archiving(handle, _)` → return `handle` (still
    functional until the cascade removes the entry)
  - On-disk `<session_id>.jsonl` exists (live, previously archived,
    OR previously replaced — archive is a runtime close, NOT a
    tombstone) → load via single-flight (§6)
  - Else → `ResumeError::NotFound`
- Disk-load enforces `max_sessions`; over-limit returns
  `ResourceExhausted`
- Restores from transcript: cwd (subject to override rule below),
  model, transcript metadata, history, usage snapshots,
  content-replacement state, tool-result references, `worktree_state`
  (if journaled)
- **cwd resolution rule (no process-cwd fallback):**
  1. If `cwd_override` is set → verify it is an existing directory
     (`std::fs::metadata(p).map(|m| m.is_dir()) == Ok(true)`, NOT
     bare `Path::exists()` which would accept regular files / symlinks
     to non-dirs). Missing or not-a-directory → return
     `ResumeError::CwdNotFound { recorded_cwd: <override> }`.
  2. Else if transcript-recorded cwd is an existing directory under
     the same `is_dir()` check → use it.
  3. Else → return `ResumeError::CwdNotFound { recorded_cwd }`. The
     caller (TUI / SDK / Hub) prompts for an explicit
     `cwd_override` and retries.
  Server never reads `std::env::current_dir()` as a fallback (per
  §13 acceptance: no session-scoped path resolution reads
  process-global cwd).
- **Truncated tail tolerance:** JSONL parser skips the final line if
  truncated (crash-recovery); logs warning, continues with prior rows
- **Dangling tool-result tolerance:** missing tool-result files resolve
  to "Result expired" placeholder strings
- Reinitializes from defaults: role overrides, app-state latches not
  transcript-backed, active turn state, permission prompt waiters,
  `session_permission_rules`
- Emits replay/bootstrap notifications tagged with `session_id`

`session/archive`:
- Requires `session_id`
- Serializes with other exclusive operations for that session
- Runs cascade per §6 archive cascade order
- Emits `session/ended` with `session_id`
- **Semantics: runtime close, not tombstone.** The transcript file
  `<session_id>.jsonl` is preserved. Subsequent
  `session/resume(session_id)` reloads from disk successfully
  (subject to `max_sessions`). The session appears in
  `session/list { archived: true }` filtered results. There is no
  v1 destructive `session/delete` RPC; if one is added later it
  must be a separate explicit RPC, not implicit in archive.
- Other connections subscribed to the same session receive
  `session/ended` and are removed from the session's reverse fan-out
  (§9.1)

`session/replace`:
- Requires `session_id` (old root session being replaced)
- Server generates new `SessionId`; clients cannot propose
- Atomic via §6 replace algorithm; no transient `max_sessions`
  over-limit
- Used by SDK `client.clear()` and TUI `/clear`
- Emits `session/started(new)` BEFORE `session/ended(old)`
- Returns `SessionReplaceResult { old_session_id, new_session_id }`
- Does NOT carry forward history, queued commands, app state,
  permission latches, cwd drift, `worktree_state`, or MCP runtime state

#### 7.1.1 Session Browse / Read API

Three-tier paginated pattern (codex-rs precedent at
`app-server-protocol/src/protocol/v2/thread.rs:937-1175`):

`session/list { cursor?, limit?, cwd_filter?, archived?, sort? }`:
- Returns paginated `SessionSummary` metadata only:
  `{ session_id, title, cwd, created_at, updated_at, turn_count, archived }`
- Opaque `cursor`; supports forward and reverse pagination
- Server default `limit` ≈ 50
- Never includes turn content (use `session/read` or `session/turns/list`)
- **`archived: bool` semantics** — the summary's `archived` flag is
  the union of "not in registry as `Ready`":
  - `SessionEntry::Ready` → `archived: false`
  - `SessionEntry::Loading` → `archived: false` (will become `Ready`
    shortly; intermediate states are not exposed)
  - `SessionEntry::Archiving` → `archived: true` (cascade is
    irreversible)
  - On-disk-only (no registry entry) → `archived: true`
  Filter `archived: true` therefore returns both in-flight
  `Archiving` cascades and disk-only sessions; `archived: false`
  returns live (`Ready` or `Loading`) sessions only.

`session/read { session_id, include_turns?: bool /* default false */ }`:
- Returns `SessionMetadata` (single session header)
- If `include_turns = true`, also returns all turns inline (for small
  sessions where pagination overhead is wasteful)
- Large sessions should call `session/turns/list` separately

`session/turns/list { session_id, cursor?, limit?, items_view?, sort? }`:
- Paginated turn body; `items_view: summary | full` controls per-turn detail
- Server default `limit` ≈ 20
- Bidirectional cursor for asc/desc traversal

This solves transcript-too-big without a single mega-RPC response.

### 7.2 Turn Requests

`turn/start`:
- Requires `session_id`
- Allocates `turn_id` (server-generated UUID; surfaced in response and
  notifications so clients can correlate streaming events with the
  correct turn, distinguish per-turn telemetry, and disambiguate
  `turn/interrupted` notifications when interrupt drops multiple turns)
- Enqueues `QueuedTurn { turn_id, token, request }` on session FIFO
- Returns `TurnStartResult { turn_id }`
- Emits `turn/started`, streaming events, completion events with
  `session_id` and `turn_id`

`turn/interrupt`:
- Requires `session_id` only; no `turn_id` parameter
- Cancels `active_turn` AND drains all `pending_turns`
- No-op when no active turn and pending queue is empty
- Non-blocking
- Emits `turn/interrupted` with `(session_id, turn_id)` per affected
  turn (one for active if present, one per queued turn dropped)

Rationale: coco-rs `pending_turns` is fed primarily by SDK callers
issuing `client.query()` without awaiting the prior turn — these
queued turns typically depend on the active turn's outcome (e.g.,
"refactor X" → "now add tests" is meaningless if the refactor was
cancelled). Cancelling all of them on user-initiated interrupt
matches "I pressed cancel, stop everything I asked for".

This differs from claude-code TS where the queue is multi-purpose
(user input + slash-command chaining + system notifications + task
completion notifications). Their queue persistence across Esc made
sense because most queued items are turn-independent. coco-rs's
narrower queue scope (SDK-only user turns) flips the default.

If a future need arises for surgical "cancel just one queued turn"
or "cancel only the active, keep queue", add a parameterized variant
later — zero protocol break.

#### 7.2.1 Turn State Machine

State per session in `SessionRuntimeHandle`:

```rust
active_turn: Mutex<Option<RunningTurn>>,
pending_turns: Mutex<VecDeque<QueuedTurn>>,
session_token: CancellationToken,   // parent of all turn tokens
```

Each `RunningTurn` / `QueuedTurn` carries a child `CancellationToken`
via `session_token.child_token()`. `session/archive` cancels
`session_token`; cascade hits all children.

`turn/start` always appends one `QueuedTurn` to `pending_turns`. The
queue drainer promotes the head when `active_turn.is_none()`. Before
promotion it checks `token.is_cancelled()`; cancelled queued turns are
dropped and emit `turn/interrupted`.

### 7.3 Request Scopes

Session-scoped (require `session_id`):

- `mcp/reconnect`, `mcp/toggle` when targeting session MCP runtime
- Model and thinking updates
- Permission-mode updates (mutates `session_permission_rules`)
- Context-usage reads
- Rewind/file-history operations
- Approval/input/elicitation `resolve` / `cancel`
- `session/list`, `session/read`, `session/turns/list` (session_id
  required on the latter two; `session/list` itself is process-level
  read)

Process-scoped (no `session_id`):

- `settings/update`
- `config/applyFlags`
- `mcp/setServers` (mutates process-wide configured server catalog)
- `plugin/reload`
- `config/read`, `config/value/write`

**Config snapshot principle:** Active sessions snapshot
`Arc<RuntimeConfig>` at `session/start` (§5.1). Process-scoped writes
affect ONLY future sessions; running sessions keep their captured
snapshot for life. To pick up new settings, archive + start (or
`session/replace`).

**Prompt ID uniqueness:** prompt IDs are server-process unique.
`session_id` is required and the server rejects resolve/cancel with
mismatched `(session_id, prompt_id)` as `WrongSession`.

Process-scoped notifications broadcast with `affected_scope: "process"`
envelope; they do not masquerade as session events.

## 8. Serialization Queues

Three scopes:

- `Session(SessionId)` — per-session FIFO; exclusive for `turn/start`,
  session-scoped MCP/runtime updates, rewind/file-history operations,
  `session/archive`, `session/replace`
- `McpOauth(server_name)` — per-MCP-server OAuth refresh; isolates auth
  flows from in-session traffic so a long-running turn cannot stall
  token refresh
- `ProcessConfig` — process-global configuration writes and
  plugin/config reloads; never held while awaiting session turns

Hot-reload (`plugin/reload`, `settings/update`) follows swap-snapshot
(§5.1) — never holds session FIFO locks. It serializes through
`ProcessConfig` only.

**Subagents do NOT participate in session FIFO.** They execute as
tool-invocations within the parent turn and use the
`StreamingToolExecutor`'s safe-concurrent / unsafe-serial model.
Cancellation of the parent turn cascades to all subagents via the
`session_token` child chain.

**Long-lived subagents register into `task_set`.** Background
`LocalAgent` spawns (`run_in_background=true` or
`AgentDefinition.background`) and `InProcessTeammate` `runner_loop`
tasks outlive the parent turn that created them. Both register their
`JoinHandle` into the session's `task_set` (§5.2) at spawn time and
descend their own `CancellationToken` from `session_token`. Archive
cascade step 6 (`task_set.shutdown().await`) is what guarantees these
drain before the entry is removed. `SendMessageTool::resume_agent`
runs inside the parent's currently-active turn (it's a tool call from
the model); the resumed spawn registers into `task_set` the same way
any other bg `LocalAgent` does.

Allowed concurrently across sessions: independent `turn/start`,
unrelated session-scoped updates, archive of session A while session B
runs. Process-global config writes serialize with each other but do not
hold any session FIFO lock.

`turn/interrupt` synchronizes through the state machine (§7.2.1) and
never blocks any queue.

FIFO behavior is part of the contract; unit tests required.

## 9. Notifications and Routing

**Routing topology:**

```
ConnectionKey → SessionId         (forward map; 1-to-1, see §14)
SessionId → HashSet<ConnectionKey> (reverse fan-out; 1-to-N)
```

One client (one `ConnectionKey`) binds to exactly one session. Multiple
clients MAY rejoin the same session via `session/resume` and share its
event stream — this is the basis for multi-client topologies such as
Hub dashboards or external monitoring tools.

Session-scoped `ServerNotification` is routed via reverse fan-out
through a typed envelope:

```rust
struct ServerEnvelope {
    session_id: Option<SessionId>,    // None = process-scoped
    agent_id: Option<AgentId>,        // Some = subagent attribution
    notification: ServerNotification,
}
```

Notification kinds:
- `session/started`, `session/ended`
- `turn/started`, `turn/completed`, `turn/failed`, `turn/interrupted`
- Message append/delta events
- Queue state events
- Compaction events
- Tool progress and tool summaries
- MCP state changes
- Permission, model, thinking, context events

**`ServerNotification::Disconnected { reason }`** is connection-scoped.
Server responsibility: graceful close — drain outbound queue, close
subscriptions, close socket. SDK responsibility: detect close
client-side and synthesize `Disconnected` into local event channel,
then terminate the channel.

When a `ConnectionKey K`'s transport closes:
1. Server removes `K` from forward map and from all reverse fan-out sets
2. Server cancels `K`'s pending outbound RPC futures (calls drain)
3. Other connections to the same session continue receiving events
4. The session itself does NOT end — `session/archive` is independent

### 9.1 Multi-client replace routing

When `session/replace(old=X)` commits (§6 Stage 2):

1. **Caller's connection** is migrated server-side: the
   `ConnectionKey K` issuing the replace had `K → X` in the forward
   map; it becomes `K → Y` atomically inside Stage 2's write-lock
   section. The caller observes `session/started(Y)` and continues on
   `Y` without further action. (This is what makes
   `client.clear()` idempotent for the calling SDK.)
2. **Other connections** in `reverse_fanout[X]` (multi-client
   subscribers — Hub dashboards, external monitors, secondary TUIs)
   receive a `session/replaced { old: X, new: Y }` notification on
   their event stream **and** are removed from `reverse_fanout[X]`.
   They are **not** auto-added to `reverse_fanout[Y]`.
3. **Peers' migration is their own choice.** A peer wanting to follow
   the replacement calls `session/resume(Y)` (which attaches them to
   `Y`'s reverse fan-out). A peer wanting to stop following lets the
   notification be the last thing it sees on X and disconnects, or
   simply ignores subsequent X-keyed RPCs (which now return `NotFound`
   once Stage 3 removes X's entry, or `ResumeError::NotFound` if X's
   transcript was never on disk).

Rationale: auto-migration would silently re-subscribe peers to a
session they may not have opted into (a Hub dashboard might be
tracking many sessions and the replacement is just one of them).
Notification-only respects the 1-client-1-session binding while
giving peers everything they need to migrate themselves.

This applies analogously to `session/archive(X)`: peers receive
`session/ended(X)` and are removed from `reverse_fanout[X]`; there is
no successor to migrate to.

**Slow-consumer protection:** outbound queue per connection is bounded
(default 1024 frames). When full, the server disconnects the slow
connection (same code path as transport close). It does NOT block the
turn emitter or other connections.

Subagent/debug notifications may carry `agent_id: Option<AgentId>`.

**OTel span propagation:** spans created under a `SessionRuntimeHandle`
carry `session_id` (and `agent_id` for subagents) as standard fields
per `common/otel/CLAUDE.md`. Each `SessionRuntimeHandle` owns a root
`session_span: tracing::Span`. Background tasks spawned via
`tokio::spawn` MUST attach via `.instrument(handle.session_span.clone())`
so span context survives the spawn boundary.

## 10. Hub Connector

Hub connector envelopes use:

    { instance_id, session_id, agent_id?, session_seq, payload }

- `instance_id` — per-server-process unique; preserved from
  `event-hub/spec.md` §4
- `session_id` — required for all session-scoped events
- `agent_id` — optional; subagent attribution
- `session_seq` — **per-session monotonic counter** (replaces v3's
  per-instance global `seq`)

Per-session seq enables O(1) replay queries: client says
"session X, last_seen_seq 42, give me everything after"; server reads
`events_for_session(X, session_seq > 42)` from index without scanning
across all sessions. Per-session ordering is preserved; cross-session
ordering is intentionally not. Sessions are independent units.

No `thread_id` appears anywhere in Hub protocol, storage DTOs, routes,
or JSONL-derived rows.

The Hub continues reading root transcripts from `<session_id>.jsonl`
and subagent transcripts from
`<session_id>/subagents/agent-<agent_id>.jsonl`.

### 10.1 Subprotocol breaking change: `coco-event-hub.v2`

The shift from a per-instance global `seq` to per-session
`session_seq` is a wire-incompatible change. The existing
`coco-hub-protocol` (`coco-rs/hub/protocol/src/lib.rs`) has:

- `AnnounceAckFrame.resume_from: Option<u64>` — single resume cursor
  per connection
- `BatchAckFrame.up_to_seq: u64` — single ack value per batch

With many sessions sharing one WS, neither can carry per-session
cursors. v2 introduces:

```rust
pub const SUBPROTOCOL_V2: &str = "coco-event-hub.v2";

#[serde(rename_all = "camelCase")]
pub struct AnnounceAckFrameV2 {
    pub first_seen: bool,
    pub hub_version: String,
    /// Per-session resume cursors. Empty map = no prior session state;
    /// missing keys = the hub has nothing on that session yet.
    pub resume_from: HashMap<SessionId, u64>,
}

#[serde(rename_all = "camelCase")]
pub struct BatchAckFrameV2 {
    /// Per-session high-water-mark of durably-stored seqs.
    pub up_to_seq: HashMap<SessionId, u64>,
}

pub struct EventEnvelopeV2 {
    pub instance_id: Uuid,
    pub session_id: SessionId,
    pub agent_id: Option<AgentId>,
    pub session_seq: u64,     // <-- per-session, replaces global seq
    pub ts: DateTime<Utc>,
    pub schema_version: u32,
    pub payload: EventPayload,
}
```

Hub-side `EventStore` indexes events by `(session_id, session_seq)`
instead of `seq`. Replay query becomes
`events_for_session(X, session_seq > resume_from[X])` — O(1) per
session via composite index, independent of total event volume across
all sessions.

**Migration:** v1 stays for read-only legacy support; new connectors
negotiate v2 via the standard subprotocol-overlap mechanism
(`event-hub/spec.md` §5.3). v1 → v2 is a hub-server change AND a
connector change; both ship in Phase 1 step 10.

## 11. Migration Sequence

No dual-stack period. New design replaces old in two phases.

### Phase 1 — Build New Stack (independent PRs, parallelizable)

1. Add this design document; mark `concurrent-app-server-plan.md` as
   superseded
2. Harden `SessionId` / `AgentId` / `McpServerName` newtypes:
   - 2a: define validators (reject path separators, dot segments,
     empty strings, platform-illegal characters)
   - 2b: scan existing transcript IDs on disk for non-conforming
     forms; fail loudly if any found (fix migration before continuing)
   - 2c: make serde fallible (`Deserialize` validates)
   - 2d: replace `From<&str>` infallible with `TryFrom<&str>`
   - 2e: change `pub struct(pub String)` to `pub struct(String)` with
     `as_str()` / `into_inner()` accessors only
   - 2f: introduce `McpServerName` typed newtype with the same
     path-safety rules; validate at `McpServerSettings` deserialization
     so a malicious `name: "../../etc/passwd"` in `mcp_servers` is
     rejected before any path-building site (PerSession PID files,
     OAuth scopes, registry keys)
3. Convert app-server-facing request/notification structs to require
   `session_id` (including approval/input/elicitation resolve/cancel)
4. Extract Process Runtime / SessionRuntimeHandle from
   `app/cli/src/session_runtime.rs` into `coco-app-runtime`:
   - 4a: introduce `McpScope::{Shared, PerSession}` and route MCP
     construction through scope decision
   - 4b: replace global MCP manager in `main.rs:360-365` with
     scope-aware factory
   - 4c: cwd audit across ~12 crates (`coco-context`,
     `coco-system-reminder`, `coco-compact`, `coco-hooks`,
     `coco-tools/file`, `coco-shell`, `coco-commands`,
     `coco-skills`, `coco-tasks`, `coco-memory`, `coco-retrieval`,
     `coco-lsp`); replace `std::env::current_dir()` with session cwd
   - 4d: implement `Arc<RuntimeConfig>` swap-snapshot for hot-reload;
     wrap process registries with `arc-swap::ArcSwap`
   - 4e: integrate `coco-coordinator` session-scoped state into
     `SessionRuntimeHandle`. PR #3 already moved 21 swarm modules
     into the `coco-coordinator` root-layer crate, but its
     session-scoped state (per-leader `SwarmMailboxHandle`, pane
     backend handles, `TeamContext`, `TeammateEntry` registry,
     per-teammate `runner_loop` task handles, `agent_handle::*`
     state) is still keyed by the legacy global "the active
     session". After this step:
     - These become fields (or owned sub-handles) on
       `SessionRuntimeHandle`. Suggested grouping:
       `coordinator_state: Option<Arc<CoordinatorSessionState>>`
       set when `Feature::AgentTeams` is enabled and at least one
       teammate has been spawned.
     - Long-lived coordinator tasks (`runner_loop` per teammate,
       mailbox watchers, pane I/O loops) register their
       `JoinHandle` into `task_set` (§5.2) and descend their
       `CancellationToken` from `session_token`.
     - Coordinator-side identity APIs continue to key by
       `(SessionId, AgentId)`; no new identity is introduced.
     - `app/cli/src/{session_runtime,tui_runner}.rs` callsites of
       `coco_coordinator::mailbox::SwarmMailboxHandle` move to
       reaching it via the leader's `SessionRuntimeHandle`.
5. Implement `coco-app-server-transport` (stdio NDJSON + UDS WS +
   TCP WS)
6. Implement `coco-app-server`:
   - 6a: private `ConnectionKey` routing (forward + reverse maps)
   - 6b: `SessionRegistry` with `SessionEntry::{Loading, Ready}` and
     single-flight resume
   - 6c: `replace` two-phase algorithm; archive cascade order
   - 6d: per-session serialization queues (`Session`, `McpOauth`,
     `ProcessConfig`)
   - 6e: enforce `max_sessions` at create + resume/load
   - 6f: transport-close graceful drain path — outbound flush,
     subscription cleanup, RPC fan-out cancellation. `Disconnected`
     itself is SDK-synthesized client-side (§14), not server-emitted.
   - 6g: slow-consumer disconnect on outbound queue full
7. Implement `coco-app-server-client` (Tier 2):
   - 7a: `LocalTransport` (typed direct, no serde) for in-process TUI
   - 7b: `RemoteTransport` (JSON-RPC) for external SDK
   - 7c: 1-client-1-session binding (§14)
   - 7d: `client.session_id()`,
     `Client::connect_with_session(opts, id)`, dual-channel
     disconnect synthesis
8. Implement `session/list`, `session/read`, `session/turns/list`
   paginated browse API
9. Implement PerSession MCP child lifecycle:
   - 9a: `PR_SET_PDEATHSIG(SIGTERM)` on Linux; kqueue parent-watch on
     macOS
   - 9b: PID file at `<session_id>/mcp-pids/<server_name>.pid` for
     crash recovery; `<server_name>` is the validated `McpServerName`
     path-safe newtype (step 2f / §16)
   - 9c: server-start orphan reaper scans all PID files; reaps dead
     PIDs and SIGTERMs live orphans
10. Hub subprotocol bump v1 → v2 (per-session seq + per-session
    `resume_from` / `up_to_seq` maps; see §10.1). Ship hub-server
    schema migration AND connector envelope change together.

### Phase 2 — Cut-over (single atomic PR)

11. Change CLI entry point: `coco sdk` → `coco serve --listen ...`
12. TUI switches to `coco-app-server-client` `LocalTransport`
13. Delete `app/cli/src/sdk_server/` entirely
14. Delete `session_id: Arc<RwLock<String>>` repointing pattern; the
    old `clear_conversation()` mutation path goes away

After Phase 2 no references to the old single-slot SDK server remain.

## 12. Test Plan

### Unit coverage

- `SessionRegistry` create/resume/archive/replace/get behavior
- Concurrent `session/resume(same_id)` single-flight: both callers
  receive same handle; no double-load
- `SessionEntry::Loading` failure path removes entry; next attempt
  retries
- `ResumeError` variants serialize/deserialize correctly
- Per-session serialization FIFO
- `turn_state` transitions (`pending_turns` / `active_turn`)
- Queued-cancel observed before promotion
- Queued-cancel observed mid-run
- Multiple queued turns run FIFO after active turn completes
- `turn/interrupt` cancels active turn AND drains pending queue
- `turn/interrupt` emits one `turn/interrupted` per affected turn
  (active if present, plus one per queued turn dropped)
- `turn/interrupt` is no-op when no active and pending queue empty
- Notification demultiplexing by `session_id` (reverse fan-out)
- Multiple clients on same session receive identical event streams
- One client's disconnect does not stop another client on the same
  session
- Valid/invalid serde round trips for `SessionId` / `AgentId`,
  including path-separator rejection
- `McpScope::Shared` and `PerSession` server lifecycle
- `max_sessions` boundary: at limit, over → `ResourceExhausted`
- `session/replace` works at `max_sessions` without transient
  over-limit
- `Disconnected` synthesized by SDK on transport close
- Per-connection `pending_requests` drain on disconnect
- Slow-consumer disconnect when outbound queue full
- `Arc<RuntimeConfig>` swap-snapshot: active session ignores mid-life
  `settings/update`; new session sees update
- Hot-reload (plugin/hook registry) does not interrupt in-flight tools
- `session_permission_rules` updates do not affect parallel sessions
- `SessionStartParams` carries no Feature overrides; all sessions
  observe the same process-resolved `Arc<Features>`
- `worktree_state` cleared on `session/replace`
- Process-global config writes serialize through `ProcessConfig` and
  do not hold session FIFO
- `TurnId` is server-generated UUID v4; appears in `turn/start`
  response, all `turn/*` notifications, and `turn/interrupted`
  fan-out (one per affected turn)
- `cwd_override` accepts a directory only; regular files and dangling
  symlinks return `ResumeError::CwdNotFound`
- Archive racing single-flight `Loading(future)`: archive awaits the
  load future; on load success it transitions `Ready → Archiving` and
  runs cascade; on load failure it returns `Ok(())` (entry already
  removed)
- `archived: true` filter returns both `Archiving` entries and
  disk-only sessions; `archived: false` returns `Ready` + `Loading`
- bg `LocalAgent` and `InProcessTeammate` `runner_loop` tasks are
  joined by `task_set.shutdown().await` on archive; the entry is not
  removed before they exit
- `ClientError::Disconnected` vs `ClientError::ClientInvalid` ordering:
  the first call hitting the dead transport returns `Disconnected`;
  every subsequent call returns `ClientInvalid` without touching the
  transport

### Integration coverage

- Two clients create two sessions concurrently
- Same session attached by two clients (both via
  `session/resume`): both see all turn events
- Transcripts remain isolated between concurrent sessions
- Archive one session while another turn runs
- Subagent writes stay under
  `<session_id>/subagents/agent-<agent_id>.jsonl`
- Background agent resume reads `(session_id, agent_id)` metadata
- Session-scoped MCP servers do not cross sessions
- Per-session cwd isolation: prompt build, hooks, reminders,
  compaction, tools for session A never read session B's cwd
- 10 concurrent sessions × short turns: throughput within K% of
  single-session baseline (perf regression)
- Disconnect mid-stream: client observes `Disconnected` event AND
  awaiting RPC future returns `TransportError`
- Reconnect via `Client::connect_with_session(opts, saved_id)`:
  successful rejoin if alive in registry; **successful reload from
  disk if archived** (archive preserves transcript); `NotFound` only
  if `<session_id>.jsonl` does not exist
- `session/resume` with missing recorded cwd and no `cwd_override`
  returns `ResumeError::CwdNotFound`; retry with explicit
  `cwd_override` succeeds; server never reads `current_dir()`
- `session/replace` Stage 1 failure leaves old fully operational
  (active turn continues, no MCP killed, no transcript flushed); only
  new's Loading entry is removed
- `session/replace` while old has an active turn: old's turn
  completes; old transitions Ready → Archiving in Stage 2; cascade
  joins the turn before removal
- `session/replaced { old, new }` notification fan-out: caller's
  ConnectionKey migrates server-side (no client-side resume needed);
  non-caller subscribers receive the notification but are NOT
  auto-attached to new
- `session/replace` consumes +1 transient slot during Stages 1-3;
  concurrent `session/start` racing at limit still gets
  `ResourceExhausted`
- TUI `/quit` blocks on `session/archive` before exit
- TUI `/resume <id>` archives current session first (transcript
  preserved), then re-opens target; no session leak
- Coordinator-mode session: archiving the leader joins all teammate
  `runner_loop` tasks before removing the entry (no orphaned panes)
- bg agent spawned in turn N persists after turn N+M completes;
  archive while bg agent is running drains it via `task_set`
- `/clear` succeeds while active session count is already
  `max_sessions`
- Wrong-session approval/input/elicitation resolve rejected as
  `WrongSession`
- PerSession MCP children terminate within grace period on archive
- PerSession MCP orphans reaped on server start (PID file present
  but process dead)
- Worktree feature: `EnterWorktree` in session A does not change
  session B's cwd
- Two sessions in same project editing same file: NO advisory lock;
  last writer wins (documented non-isolation, not a guarantee)
- OTel `session_id` field appears on all spans emitted under a
  session, including from `tokio::spawn` background tasks
- `session/list` / `session/read` / `session/turns/list` paginate
  correctly with bidirectional cursors

Verification: run `just quick-check` during iteration; run
`just pre-commit` once before commit.

## 13. Acceptance Criteria

- One server process hosts multiple root sessions concurrently
- Every session-scoped RPC carries explicit `session_id`; no implicit
  defaults on wire or in transport state
- Server internals use private `ConnectionKey` for routing/draining;
  no connection identity in public protocol or persistence
- `SessionId` / `AgentId` are validated path-safe newtypes; private
  inner field; fallible serde
- No root session observes another's history, command queue, MCP
  state, active turn, transcript writes, or session-tier permission
  rules
- No session-scoped path resolution, prompt assembly, hook context,
  reminder source, compaction path, or tool default reads
  process-global cwd
- SDK/TUI clients demultiplex session-scoped notifications by
  `session_id` (reverse fan-out)
- Multiple clients on the same session share its event stream;
  disconnect of one does not stop another or end the session
- Subagent persistence unchanged; keyed by `(session_id, agent_id)`
- Subagents inherit session's resolved `Arc<Features>` (post-override)
  and may only narrow further
- Public API, disk layout, SDK schema, Hub envelopes contain no
  `ThreadId`
- `session/resume(session_id)` is idempotent across rejoin and disk
  load, including concurrent same-id resume races (single-flight)
- `Disconnected` reaches both event-stream subscribers (SDK-synthesized)
  and in-flight RPC callers
- `max_sessions` enforced; over-limit `session/start` returns
  `ResourceExhausted` (no eviction); disk-load resume counts against
  the same limit; replace does not transiently consume extra capacity
- MCP isolation modes selectable per server; PerSession servers do
  not leak across sessions; orphaned PerSession children are reaped
  on server start
- OTel spans carry `session_id` (and `agent_id` for subagents);
  background-spawned tasks preserve span context via `.instrument()`
- Hub envelopes carry `instance_id + session_id + agent_id? +
  session_seq`; zero `thread_id` occurrences
- `session_seq` is per-session monotonic
- Hot-reload uses swap-snapshot: in-flight turns keep prior
  registry / config; only post-reload lookups see new version
- `Arc<RuntimeConfig>` snapshot at `session/start` is immutable for
  session lifetime
- `session/start` does not accept Feature overrides; Features resolve
  once at process boot per root `CLAUDE.md` resolution layers
- `turn/interrupt` takes only `session_id`; cancels active turn AND
  drains pending queue; each affected turn (active + queued) gets its
  own `turn/interrupted` notification
- `session/list` / `session/read` / `session/turns/list` paginate;
  `session/read` does not return full turn body by default
- Cross-session file-system access is NOT isolated (documented
  non-goal; user responsibility)
- Worktree feature gated by `Feature::Worktree` only — no independent
  session/start parameter
- `coco-app-server-client` public API is Tier 2 (thiserror); no
  `coco-error` dependency leaks to SDK users
- In-process Client uses `LocalTransport`: typed values bypass JSON
  serde on the hot path
- Background sinks hold `Weak<SessionRuntimeHandle>`; archive
  completes via explicit `task_set.shutdown().await` (JoinSet drain) —
  never blocks on `Arc::strong_count`
- `task_set` is part of `SessionRuntimeHandle`'s field surface;
  bg subagents, teammate `runner_loop`s, mailbox watchers, PerSession
  MCP supervisors, transcript-flush writer, and periodic-summary
  timers all register at spawn
- `TurnId` is a typed newtype (`pub struct TurnId(String)`) with
  `TryFrom<&str>` validation; appears in `turn/start` response, all
  `turn/*` notification envelopes, and OTel per-turn spans
- `ClientError` is a closed thiserror enum with variants
  `Connect`, `Disconnected`, `ClientInvalid`, `Server`, `Timeout`,
  `InvalidArgument`; not `Clone` (single-owner Client)
- `session/list { archived: true }` covers both `Archiving` and
  disk-only sessions; `archived: false` covers `Ready` + `Loading`
- `cwd_override` validation uses `metadata.is_dir()`; bare
  `Path::exists()` is insufficient (would accept regular files)
- `archive` on `Loading` entry awaits the load future first; no
  registry lock held across the await
- `coco-coordinator` session-scoped state (mailbox handles, pane
  handles, runner_loop task handles) lives under
  `SessionRuntimeHandle` (or sub-handles owned by it); coordinator
  long-lived tasks register into `task_set` and descend their tokens
  from `session_token`
- `session/replace` is atomic in the sense that any Stage 1 failure
  leaves old fully operational; Stage 2 is a single write-lock
  section with no `.await`; Stage 3 cascade is idempotent
- `session/replace` bypasses `max_sessions` by +1 transiently;
  concurrent `session/start` still sees the full limit
- Multi-client `session/replaced` notification routes to non-caller
  subscribers in old's reverse fan-out; auto-migration is NOT
  performed (peers must call `session/resume(new)` themselves)
- Archived sessions reload via `session/resume` (archive is runtime
  close, not tombstone); `session/list` exposes them under
  `archived: true` filter
- `session/resume` with stale recorded cwd requires explicit
  `cwd_override` (no `current_dir()` fallback)
- Hub subprotocol v2 carries per-session `session_seq` and
  per-session `resume_from` / `up_to_seq` maps; v1 frames remain
  parseable for legacy read-only paths only
- Slow-consumer disconnect: a stuck consumer is dropped, not allowed
  to block turn emission
- Old single-slot SDK server entirely deleted after Phase 2

## 14. SDK Client Contract

`coco-app-server-client` (Tier 2: thiserror; `ClientError` is
crate-local; no `coco-error` dependency in the public API) exposes a
1-client-1-session binding.

**`ClientError` variants** (all returned by the API methods below):

```rust
#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    /// Initial transport / handshake failure (DNS, refused, TLS, …).
    #[error("connection failed: {0}")]
    Connect(String),

    /// Transport closed mid-session. Synthesized by the SDK transport
    /// task; ALSO resolves every in-flight RPC future (dual-channel).
    #[error("transport disconnected")]
    Disconnected,

    /// Any call after `Disconnected` short-circuits without touching
    /// the transport. Recovery = drop + `connect_with_session`.
    #[error("client invalid (reconstruct via connect_with_session)")]
    ClientInvalid,

    /// Server returned a JSON-RPC error response.
    #[error("server error {code}: {message}")]
    Server { code: i32, message: String },

    /// RPC future awaited beyond the per-call deadline.
    #[error("request timed out")]
    Timeout,

    /// Caller passed a malformed argument (e.g. empty session_id).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
```

Not `Clone` (Client is single-owner). On disconnect, the SDK drains
`pending_requests` and resolves each pending future with
`Err(Disconnected)`; subsequent calls return `Err(ClientInvalid)`.

- `Client::connect(opts) -> Result<Client, ClientError>` (async,
  fallible): internal `session/start`, caches `session_id`
- `Client::connect_with_session(opts, session_id) -> Result<Client,
  ClientError>` (async, fallible): internal `session/resume`; also
  the user-driven post-disconnect recovery API
- `client.session_id() -> SessionId` (sync getter): user captures
  before any disconnect risk
- `client.query(...) -> Result<TurnId, ClientError>`: injects cached
  `session_id` into `turn/start`; returns `turn_id` for tracking
- `client.interrupt() -> Result<(), ClientError>`: cancels active
  turn AND drains pending queue; no parameters
- `client.clear() -> Result<(), ClientError>`: atomic replace
  1. `new_id = session/replace`
  2. swap cached `session_id = new_id`
  3. old session archive completes as part of replace
- Transport disconnect → Client transitions to invalid state; all
  subsequent calls return `ClientError::ClientInvalid`. No auto-
  reconnect, no resume retries.
- Recovery (user owns the policy):

      let saved_id = client.session_id();
      // ... disconnect ...
      drop(client);
      let client = CocoClient::connect_with_session(opts, saved_id).await?;

- `client.close() -> Result<(), ClientError>` (explicit, async):
  `session/archive` then drop transport
- `Drop` is silent. Users MUST call `client.close().await?` to clean
  up the session.

**LocalTransport vs RemoteTransport.** In-process clients (TUI) use
`LocalTransport`, which passes typed values WITHOUT JSON serialization
(zero serde overhead on the hot path: turn streaming events). The
public Client API is identical between transports — implementation
differs at the transport layer only.

Disconnect uses dual-channel signaling (codex-rs precedent at
`app-server-client/src/remote.rs:241, 281-328`):

1. **Event stream** — `ServerNotification::Disconnected` is synthesized
   by the SDK's transport task on socket-close detection; streaming
   consumers observe naturally
2. **In-flight RPCs** — every pending RPC future resolves immediately
   with `Err(ClientError::Disconnected)`; SDK drains
   `pending_requests` map

Both channels: RPC-only leaves streaming receivers hanging;
event-only leaves awaiting RPC futures leaked.

## 15. TUI Behavior Contract

- `AppState.current_session_id: SessionId` (always set after startup)
- Startup → `Client::connect(SessionStartParams { cwd, ... })` →
  set `current_session_id`
- `/clear` → `client.clear()` (atomic replace; updates cached
  `session_id` internally)
- `/resume <id>` → `client.close().await` (blocking; calls
  `session/archive` on the current session — transcript preserved on
  disk, re-openable later) → `Client::connect_with_session(opts, id)`
  (re-opens target from disk if not already in registry). Without the
  explicit close, repeated `/resume` would accumulate live sessions
  in the registry (no server auto-GC) and eventually hit
  `max_sessions`.
- `/quit` → `client.close()` (blocking; calls `session/archive`) →
  exit

Multi-window UI is out of scope. Future evolution to
`HashMap<SessionId, Client> + active` requires zero protocol change.

## 16. MCP Isolation Modes

`McpScope::{Shared, PerSession}` per server, configured in
`mcp_servers[].scope`.

- `Shared` (default for stateless servers: REST, local CLI) — server
  lives process-wide; requests carry `(session_id, payload)` and route
  to the same connection
- `PerSession` (opt-in for stateful servers: per-session process state,
  working directory, temp files, or DB connections) — fresh server
  process per root session; dies with the session

**PerSession lifecycle:**

- Server spawn: set `PR_SET_PDEATHSIG(SIGTERM)` on Linux; kqueue
  parent-watch on macOS
- PID file at `<session_id>/mcp-pids/<server_name>.pid` where
  `<server_name>` is the validated `McpServerName` newtype (§2 / §11
  step 2f): same path-safety rules as `SessionId` — rejects `/`,
  `\`, `.`, `..`, platform separators, empty strings; private inner
  field; fallible serde. Validation runs at `McpServerSettings`
  deserialization, so config-supplied names cannot reach any
  path-building site without being validated first.
- `session/archive` cascade step 4: SIGTERM → 5s grace → SIGKILL
- Server-process startup: scan all `<session_id>/mcp-pids/` dirs; for
  each PID, check if alive; if dead, remove PID file; if alive but
  parent dead (orphan from prior crash), SIGTERM and remove

**OAuth credentials remain process-global** per server configuration in
v1. Therefore `PerSession` does NOT mean per-session OAuth identity.
If a future server needs per-session OAuth accounts, add an explicit
`McpAuthScope::PerSession` and key token storage / refresh queues by
`(session_id, server_name)`.

OAuth refresh is per auth scope. In v1 that means `McpOauth(server_name)`
queue serializes all OAuth refresh for one server across all PerSession
instances. Burst contention possible during simultaneous token expiry
across many PerSession instances; acceptable for v1.

## 17. Configuration

`max_sessions: usize`
- Default 32
- Source order: `--max-sessions` CLI > `COCO_MAX_SESSIONS` env >
  `~/.coco/config.json:server.max_sessions` > built-in default
- Over-limit behavior: `session/start` returns `ResourceExhausted`; no
  eviction
- `session/resume` requiring disk-load also enforces this limit
- `session/replace` **bypasses `max_sessions` by +1 for its own
  duration** (Stage 1 through Stage 3 of the replace algorithm, §6).
  This is a swap, not a new capacity grant: a concurrent
  `session/start` racing during replace still sees the full limit
  and may receive `ResourceExhausted`.
- Register `MaxSessions` variant in `coco_config::EnvKey`; never call
  `std::env::var` ad-hoc

**Configuration ownership:**

- Process-global (no per-session overlay in v1):
  - `RuntimeConfig` (model providers, paths, MCP server catalog, ...)
  - Plugin / skill / hook / command registries
  - Feature gates (resolved once at process boot; no per-session
    override layer in v1)
  - Settings persistence (`settings.json` writes)
- Session-scoped (lives in `SessionRuntimeHandle`):
  - `cwd`, history, app state, active/pending turns, attachment inbox,
    session memory, usage tracker, per-session MCP runtime handles
    (per `McpScope`), `session_permission_rules`, `worktree_state`,
    `Arc<Features>` clone (same value across all sessions in the
    process)

`Arc<RuntimeConfig>` is snapshot-captured per session at `session/start`;
`settings/update` does NOT affect running sessions.

**Worktree feature** is governed entirely by the process-level
`Feature::Worktree` gate.
- Source: `settings.json:worktree.enabled`, `COCO_FEATURE_WORKTREE`
  env, or CLI flag — resolved once at process boot per the standard
  Features-resolution layers (root `CLAUDE.md`)
- All sessions in the process share the same `Feature::Worktree`
  value; SDK has no per-session override path
- When `Feature::Worktree = true`, the session's model and subagents
  can call `EnterWorktree` / `ExitWorktree` tools and use
  worktree-isolated subagents (`AgentWorktreeConfig`)
- When `Feature::Worktree = false`, those tools are filtered out by
  the `Tool::is_enabled` gate; the model cannot invoke them
- `WorktreeConfig.symlink_directories` and similar advanced sub-config
  remain process-level only in v1; no `session/start` parameter
- Per-session worktree state lives in
  `SessionRuntimeHandle.worktree_state` (§5.2) — written by
  `EnterWorktree`, cleared by `ExitWorktree`, restored from transcript
  metadata on `session/resume`

## 18. Decision Log

Locked decisions from design iteration (numbered for cross-reference):

| # | Decision |
|---|---|
| 1 | `session_id` mandatory on every session-scoped RPC; no implicit defaults |
| 2 | Server-generated `SessionId` (UUID v4); clients cannot propose |
| 3 | `session/resume(session_id)` idempotent (rejoin running OR load from disk) |
| 4 | `/clear` discards all state; no `carry_forward` |
| 5 | `session/replace` atomic primitive (no `session/reset` RPC) |
| 6 | 1 client binds to 1 session; multiple clients MAY rejoin same session via resume |
| 7 | TUI `/quit` auto-archives; SDK `Drop` silent (`close()` explicit) |
| 8 | `max_sessions` configurable (default 32); over-limit `ResourceExhausted`, no eviction |
| 9 | Disconnect = dual-channel (`Disconnected` event synthesized client-side + RPC error) |
| 10 | Client invalid after disconnect; recovery = new `connect_with_session(opts, id)` |
| 11 | `client.session_id()` getter for user-driven resume |
| 12 | `SessionId` / `AgentId` validated path-safe newtypes (private fields, fallible serde) |
| 13 | `ConnectionKey` private; never on wire/disk |
| 14 | `SessionEntry::Loading` single-flight prevents double-load of same id |
| 15 | `replace` algorithm two-phase **commit**: Stage 1 builds new fully (old untouched); Stage 2 atomic commit (single write-lock section, no await); Stage 3 background archive. Stage-1 failure rolls back cleanly. `max_sessions` bypassed by +1 for replace's own duration (swap, not capacity grant). |
| 16 | `cwd` restored from transcript metadata on resume. **No `current_dir()` fallback** — `SessionResumeParams.cwd_override: Option<PathBuf>` is required when recorded cwd is missing; otherwise `ResumeError::CwdNotFound`. |
| 17 | No per-session config overlays; process config is global |
| 18 | MCP `PerSession` ≠ per-session OAuth (v1) |
| 19 | Approval `(session_id, prompt_id)` mismatch rejected as `WrongSession` |
| 20 | Hub `session_seq` per-session monotonic (not per-instance global) |
| 21 | `Disconnected` synthesized client-side by SDK transport task on socket close |
| 22 | No dual-stack migration; rip-and-replace in two phases |
| 23 | `session/list` / `session/read` / `session/turns/list` three-tier paginated |
| 24 | Worktree fully via `Feature::Worktree`; no independent `session/start` param |
| 25 | No per-session Feature overrides; all sessions share process-resolved `Arc<Features>` |
| 26 | Cross-session file-system isolation NOT guaranteed (documented non-goal) |
| 27 | Process state uses snapshot-at-session-start: every `session/start` clones the current `Arc<T>` for each piece (RuntimeConfig, registries, …) into the `SessionRuntimeHandle`. Sessions never re-read process slots. Process-level mutation uses `arc-swap::ArcSwap::store` / `tokio::sync::watch::send` (NOT `Arc::swap` — that's not a Rust API; v4 wording was incorrect). Mid-session `settings/update` / `plugin/reload` has zero effect on running sessions. |
| 28 | `coco-app-server-client` Tier 2 (thiserror), not Tier 3 |
| 29 | In-process Client uses `LocalTransport` (zero serde on hot path) |
| 30 | Background sinks hold `Weak<SessionRuntimeHandle>` (not `Arc`) |
| 31 | Archive cascade order: cancel `session_token` → drop pending queue → drain active turn → MCP SIGTERM/SIGKILL → flush transcript → `task_set.shutdown().await` (explicit JoinSet drain) → remove entry. **No `Arc::strong_count` polling** — that pattern is paradoxical (archive itself holds a strong ref) and brittle (any un-audited `Arc::clone` blocks forever). |
| 32 | Turn state: `active_turn: Option<RunningTurn>` + `pending_turns: VecDeque<QueuedTurn>` |
| 33 | `turn/interrupt { session_id }` cancels active + drains pending queue; no `turn_id` (queued turns typically depend on active turn) |
| 34 | Subagents NOT in session FIFO; execute as tool invocations within parent turn |
| 35 | OTel `session_span` field; spawned tasks use `.instrument(span.clone())` |
| 36 | Slow-consumer disconnect on outbound queue full (default 1024 frames) |
| 37 | JSONL truncated-tail / dangling tool-result tolerance on resume |
| 38 | `turn_id` server-generated and surfaced for streaming-event correlation and per-turn telemetry/notification routing |
| 39 | PerSession MCP children: PDEATHSIG (Linux) / kqueue (macOS) + PID file + startup orphan reaper |
| 40 | `Arc<RuntimeConfig>` snapshot at `session/start`; immutable for session lifetime |
| 41 | `session/archive` is **runtime close, not tombstone**. Transcript file preserved; `session/resume(id)` reloads archived sessions from disk; `session/list { archived: true }` lists them. No v1 `session/delete` RPC. |
| 42 | Multi-client `session/replace` routing: caller's `ConnectionKey` migrates server-side atomically in Stage 2; non-caller subscribers receive `session/replaced { old, new }` notification + removal from old's fan-out, but are **not auto-attached** to new (each peer decides via `session/resume(new)`). |
| 43 | Hub subprotocol breaking change v1 → **v2** for per-session seq: `AnnounceAckFrameV2.resume_from: HashMap<SessionId, u64>` and `BatchAckFrameV2.up_to_seq: HashMap<SessionId, u64>` replace the v1 per-connection single-value fields (§10.1). |
| 44 | `McpServerName` typed path-safe newtype (same rules as `SessionId`/`AgentId`). Validated at `McpServerSettings` deserialization; reaches every path-building site (PID files, OAuth scopes) only post-validation. |
| 45 | TUI `/resume <id>` archives current session first (`client.close().await` → `session/archive`) before re-opening target. Without this, repeated `/resume` accumulates live sessions in the registry (Drop is silent; no server auto-GC) and eventually trips `max_sessions`. |
| 46 | `SessionEntry` has three states: `Loading(SharedLoadFuture)` / `Ready(Arc<Handle>)` / `Archiving(Arc<Handle>, SharedArchiveFuture)`. All three count toward `max_sessions`; replace's +1 transient bypass is the single exception. |
| 47 | `TurnId` is a typed newtype (`pub struct TurnId(String)`, UUID v4, serde-as-string). Server-generated per turn; surfaced in `turn/start` response, all `turn/*` notifications, and OTel per-turn spans. NOT a path component (no `.`/`/` validation needed). |
| 48 | `coco-coordinator` session-scoped state (per-leader `SwarmMailboxHandle`, pane backend handles, `TeamContext`, `TeammateEntry` registry, per-teammate `runner_loop` task handles, `agent_handle` state) is integrated into `SessionRuntimeHandle` (or a `coordinator_state: Arc<CoordinatorSessionState>` sub-handle owned by it). All long-lived coordinator tasks register into `task_set` and descend their `CancellationToken` from `session_token` so archive cascade drains them. Identity APIs still key by `(SessionId, AgentId)`; no new identity. |
| 49 | Three subagent task-kind lifecycles. `LocalAgent` foreground = in-turn tool invocation, cancelled by parent turn token, not in `task_set`. `LocalAgent` background (`run_in_background=true` / `AgentDefinition.background`) = outlives parent turn, joined into `task_set`. `InProcessTeammate` = long-lived teammate with `runner_loop` per teammate, joined into `task_set`. `SendMessageTool::resume_agent` runs inside the parent turn (it's a model tool call); the resumed spawn joins `task_set` as bg. |
| 50 | Archive on `SessionEntry::Loading` awaits the in-flight load future before running cascade. Load failure → archive returns `Ok(())` (entry already removed by single-flight failure path). Load success → transition `Ready → Archiving` and run cascade on the resulting handle. Await happens outside the registry lock. |
| 51 | `cwd_override` validation uses `std::fs::metadata(p).map(|m| m.is_dir()) == Ok(true)`, NOT bare `Path::exists()`. Same `is_dir()` check applies to the transcript-recorded cwd fallback. Regular files / dangling symlinks / files-as-paths return `ResumeError::CwdNotFound`. |
| 52 | `session/list { archived: bool }` filter semantics: `false` covers `Ready` + `Loading` (live or about-to-be-live); `true` covers `Archiving` (cascade is irreversible) + disk-only sessions. The summary's `archived: bool` is the union of "not in registry as `Ready`-or-`Loading`". |
| 53 | SDK `ClientError` is a closed thiserror enum with variants `Connect(String)`, `Disconnected`, `ClientInvalid`, `Server { code, message }`, `Timeout`, `InvalidArgument(String)`. NOT `Clone` (single-owner Client). On transport close, the SDK transport task drains `pending_requests` and resolves each pending future with `Err(Disconnected)`; subsequent calls return `Err(ClientInvalid)` without touching the transport. |
| 54 | **Teammate-as-session promotion is v2 future direction, wire-forward-compatible from v1.** v1 keeps `InProcessTeammate` teammates owned by `coco-coordinator` under `<leader_session_id>/subagents/`. v2 (if pursued) introduces a `TeamId` identity, cross-session mailbox primitive keyed by team, and disk layout migration to `<teammate_session_id>.jsonl`. The v1 wire surface (`session/start` extensibility, per-session `session_seq`, `Client::connect_with_session`) is forward-compatible; v2 is a disk + identity change, not a wire break. |
