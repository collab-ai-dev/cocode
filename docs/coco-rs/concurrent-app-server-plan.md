# Concurrent App-Server Plan for coco-rs

Status: design (proposed). Mirrors `codex-rs/app-server/` architecture into coco-rs.
Author: review-ready first draft. Companions: see [event-hub/spec.md](event-hub/spec.md), [agentteam-architecture.md](agentteam-architecture.md), [multi-provider-plan.md](multi-provider-plan.md).

## 0. Context

`coco-rs` today is **single-session per process**. `app/cli/src/sdk_server/handlers/mod.rs:239` defines
`SdkServerState { session: RwLock<Option<SessionHandle>>, … }` — the comment is explicit: *"Only one
concurrent session per server — mirrors TS where `structuredIO.ts` holds a single `currentSession`
slot."* The SDK transport is stdio NDJSON only; there is no `HashMap<ConnId, ConnState>`, no
`ThreadId`, no per-request routing, no isolation of MCP connections or PTY pools between
conversations. `app/query::QueryEngine` is even **rebuilt per turn** (looser than codex's per-session
ownership).

`codex-rs/app-server/src/lib.rs:733-1060` is the proven reference for a true multi-client concurrent
server: outbound router task + inbound processor task (`HashMap<ConnectionId, ConnectionState>`),
per-request `tokio::spawn` for the common path, `RequestSerializationQueue` for scoped serial
operations, per-thread `Session` owning its `McpConnectionManager` / `UnifiedExecProcessManager` /
`active_turn`, per-turn `TurnContext` snapshot for cwd/sandbox/permission.

This document is the system design plan for porting that architecture into coco-rs and **deleting
the single-session SDK loop wholesale** (user-confirmed: disregard backward compatibility).
User-confirmed scope: **WebSocket + Unix Domain Socket + Stdio NDJSON** transports, **per-thread MCP
connections** (codex parity, with `mcp.isolation_mode` opt-out), **Hub stays separate** (1 connector
per process, multiplexed by `ThreadId`).

## 1. Identity Model — Two Levels, Not a Rename

> **Earlier mistake (corrected).** A first pass proposed renaming `coco_session::SessionId` to
> `ThreadId` workspace-wide as a single identity. That conflates two genuinely distinct concepts in
> codex-rs. The correct model has **both**.

### 1.1 The codex-rs identity types (ground truth)

`codex-rs/protocol/src/thread_id.rs:11-29` and `codex-rs/protocol/src/session_id.rs:15-65`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, TS, Hash)]
#[ts(type = "string")]
pub struct ThreadId  { pub(crate) uuid: Uuid }   // UUID v7

#[derive(Debug, Clone, Copy, PartialEq, Eq, TS, Hash)]
#[ts(type = "string")]
pub struct SessionId { pub(crate) uuid: Uuid }   // UUID v7

impl From<ThreadId> for SessionId { /* same UUID inside */ }
impl From<SessionId> for ThreadId { /* same UUID inside */ }
```

Both are UUID-v7 newtypes with **lossless** `From` conversions. They are **distinct types at the
Rust level** but interconvertible by value.

The wire-protocol `Thread` (codex's `app-server-protocol/src/protocol/v2/thread_data.rs:105`) carries
**both** plus a fork link:

```rust
#[ts(export_to = "v2/")]
pub struct Thread {
    pub id: String,                              // ThreadId
    /// Session id shared by threads that belong to the same session tree.
    pub session_id: String,                      // SessionId
    /// Source thread id when this thread was created by forking another thread.
    pub forked_from_id: Option<String>,          // parent ThreadId on fork
    // …  preview / cwd / model_provider / status / timestamps …
}
```

### 1.2 What each ID actually means

| Concept | Type | Semantics |
|---|---|---|
| **Thread** | `ThreadId` | One conversation. One rollout JSONL file (`<thread_id>.jsonl`). One `Session` runtime + one `submission_loop` task. Immutable for the thread's lifetime; persisted on disk. |
| **Session** | `SessionId` | A grouping key. **Inherited by sub-agents** from their root thread. For root threads created via `thread/create`, `SessionId = SessionId::from(thread_id)` (same UUID). For sub-agents, `SessionId = parent.session_id`. |
| **Fork link** | `forked_from_id: Option<ThreadId>` | When `thread/fork` creates a child from a parent, the child gets a **new `ThreadId` AND a new `SessionId`** — fork breaks out of the parent's session tree. `forked_from_id` records the source for audit/UI. |

### 1.3 Concrete allocation rules (from `codex-rs/core/src/session/session.rs:951-955`)

```rust
let session_id = if session_configuration.session_source.is_non_root_agent() {
    agent_control.session_id()              // sub-agent INHERITS parent session_id
} else {
    SessionId::from(thread_id)              // root thread: session_id derived from thread_id
};
```

Three creation paths, three identity outcomes:

```text
┌──────────────────────────────────────────────────────────────────────────┐
│  thread/create                                                            │
│    ThreadId  = new()                                                      │
│    SessionId = SessionId::from(ThreadId)         ← same UUID              │
│    forked_from_id = None                                                  │
├──────────────────────────────────────────────────────────────────────────┤
│  spawn_subagent (within a turn of parent T_root)                         │
│    ThreadId  = new()                              ← distinct conversation │
│    SessionId = T_root.session_id                  ← INHERITED             │
│    forked_from_id = Some(T_root.id)               ← optional, for trace   │
├──────────────────────────────────────────────────────────────────────────┤
│  thread/fork(source = T_old, snapshot = TruncateBeforeNthUserMessage(n)) │
│    ThreadId  = new()                                                      │
│    SessionId = SessionId::from(ThreadId)          ← NEW session tree      │
│    forked_from_id = Some(T_old.id)                ← audit only            │
└──────────────────────────────────────────────────────────────────────────┘
```

### 1.4 What `/new`, `/clear`, `/fork`, `/compact` map to in codex-rs

The user asked: "can one thread sequentially contain multiple sessions, e.g. on `/clear`?"

**Answer: No — and we should not introduce that model.** In codex-rs there is no "new session
within the same thread" operation. Verified by walking the actual TUI → app-server → core chain
(`tui/src/chatwidget/slash_dispatch.rs:167-169`, `tui/src/app/event_dispatch.rs:30-41`,
`tui/src/app/session_lifecycle.rs:463-528`,
`app-server/src/request_processors/thread_processor.rs:1100-1105`,
`core/src/session/session.rs:515-520`):

| Slash | Wire / Op | `ThreadId` | `SessionId` | `forked_from_id` | History | Notes |
|---|---|---|---|---|---|---|
| `/new` | `thread/start` + `ThreadStartSource::Startup` | new UUID | new UUID | `None` | empty | fresh thread |
| `/clear` | `thread/start` + `ThreadStartSource::Clear` | new UUID | new UUID | `None` | empty | **identical to `/new` at the identity level**, only difference is a hook tag (`SessionStartSource::Clear`) so `SessionStart` hooks + analytics can distinguish, plus the TUI clears the terminal first |
| `/fork` | `thread/fork` + `ForkSnapshot` | new UUID | new UUID | `Some(parent.id)` | truncated copy of parent | new session tree, but `forked_from_id` link records lineage |
| `/compact` | `Op::Compact` (op on current thread, not RPC) | unchanged | unchanged | unchanged | compressed in place; appends `RolloutItem::Compacted` to rollout |
| (rollback last N) | `Op::ThreadRollback { N }` | unchanged | unchanged | unchanged | last N dropped from memory; disk untouched |

The smoking gun in codex (`core/src/session/session.rs:515-520`):

```rust
let thread_id = match &initial_history {
    InitialHistory::New | InitialHistory::Cleared | InitialHistory::Forked(_) => {
        ThreadId::default()                              // fresh UUID v7
    }
    InitialHistory::Resumed(rh) => rh.conversation_id,
};
```

`InitialHistory::Cleared` shares the `ThreadId::default()` branch with `New` — so a `/clear` thread
is **a brand-new thread**, not "the same thread with cleared history".

The full `/clear` lifecycle (from the TUI):

1. `SlashCommand::Clear` → `AppEvent::ClearUi`.
2. `clear_terminal_ui()` + `reset_app_ui_state_after_clear()` — visual + UI-state reset.
3. `shutdown_current_thread(app_server).await` — **the previous `Thread` runtime is shut down**;
   the TUI unsubscribes from its event stream.
4. `app_server.start_thread_with_session_start_source(&config, Some(ThreadStartSource::Clear))` —
   request a brand-new `Thread`.
5. TUI prints `To continue this session, run codex --resume <old_thread_id>` — the previous thread's
   rollout JSONL stays on disk; the user can resume it.

**Reasons to mirror codex here in coco-rs:**

1. **Persistence is per-`ThreadId`.** One JSONL file per thread. If `/clear` reused the same
   `ThreadId`, we'd either truncate the file (lose audit) or invent intra-file segment markers
   (parser complexity).
2. **The per-thread `Session` runtime is single-active-turn**, with a single `submission_loop`
   task. Allowing two "sessions" sequentially under one thread would either reuse the runtime
   (state cleanup hell) or replace it (effectively a fresh thread anyway).
3. **The current `Arc<RwLock<String>> session_id` in coco's `session_runtime.rs:474` IS the bug.**
   It rotates on `/clear`, which is exactly what makes `(thread_id, session_id)` undisciplined and
   confusing. Mirroring codex's model — immutable `ThreadId` per `Thread`, **`/clear` spawns a new
   `Thread`** — removes the rotation cleanly.

The 3-level structure (Process → Session-tree → Thread) gives us all the expressive power without
needing intra-thread session sequencing.

### 1.5 Why both IDs — physical execution unit vs logical conversation

The two IDs answer two genuinely different questions:

| Question | Answered by |
|---|---|
| "Which independent runtime is this? — own event loop, own rollout JSONL, own MCP boundary, own active_turn slot." | **`ThreadId`** (physical execution unit) |
| "Which user-visible conversation is this? — what the user calls 'my chat', telemetry pivots, quota lines, OAuth/policy sharing scope." | **`SessionId`** (logical conversation identity) |

**Sub-agents force the two apart.** A sub-agent **must** be a new physical unit:

- Own `submission_loop` task — can't block the parent's turn driver
- Own rollout JSONL — otherwise parent transcript replay produces interleaved garbage
- Own `active_turn: Mutex<Option<ActiveTurn>>` — sub-agent runs concurrently with parent
- Own `McpConnectionManager` (in per-thread isolation mode) — MCP server crashes don't propagate

…but the sub-agent is **logically** the same conversation — same user, same task, same telemetry
group, same OAuth/permission policy as the root. Hence: **fresh `ThreadId`, inherited `SessionId`**.

`/fork` is the opposite asymmetry — it's a **deliberate new conversation** that happens to start
from a snapshot of an old one. Logically new (new `SessionId`), physically new (new `ThreadId`).
`forked_from_id` records the lineage for audit.

`/clear` is also a deliberate new conversation — fresh `ThreadId` AND fresh `SessionId`, with a
hook tag (`SessionStartSource::Clear`) so analytics can tell `/clear` apart from `/new`.

**This is why "collapse to one ID" is the wrong simplification.** It would force one of:
- Sub-agents run in the parent's runtime → can't parallelize, breaks isolation, defeats the whole point
- Sub-agents are fully independent (no shared id) → breaks telemetry / OAuth / policy sharing → SDK
  shows one "conversation" while server bills it as N separate ones

Codex's split — physical (Thread) vs logical (Session) — captures the only model that supports both
parallelism and shared identity.

### 1.5.1 What the SDK sees vs what the server runs

Sub-agents are **server-internal**. The SDK surface exposes only `SessionId`:

| Surface | Concept exposed | Why |
|---|---|---|
| SDK (`coco-sdk` Python, IDE plugins) | **`session_id` only** | User mental model: "one conversation". Sub-agents are transparent implementation detail. |
| Wire (`session/*` JSON-RPC methods between SDK and app-server) | **`session_id` in params; events tagged `session_id`** | Mirror SDK. Never leak `thread_id` to clients. |
| Server internals (`Thread`, `ThreadManager`, rollout filenames) | **both `ThreadId` (primary) + `SessionId` (grouping)** | Internal implementation; physical isolation needs `ThreadId`. |
| Hub envelope (process → Hub server) | **both `thread_id` + `session_id`** (D11) | Operator tooling needs the thread tree for debugging sub-agent behavior. |
| Spawned-process env (`exec/shell`, `exec/mcp`) | **both `COCO_SESSION_ID` (default for hook scripts) + `COCO_THREAD_ID` (advanced)** | Hook scripts want logical grouping; advanced telemetry wants physical attribution. |

**Implication for `coco-sdk`**: the existing `session_*` API surface (`SessionNotFoundError`,
`client.read_session(session_id)`, `_session_id`, `session/resume` wire method) is **already
correctly named**. No big SDK refactor needed — only schema regen + wire method version bump.

### 1.6 Where each ID lives in coco-rs

```text
ProcessRuntime
└── ThreadManager
    ├── threads:           RwLock<HashMap<ThreadId, Arc<Thread>>>      ← physical lookup (every msg)
    └── sessions:          RwLock<HashMap<SessionId, Vec<Weak<Thread>>>> ← logical grouping (Hub UI / telemetry)
        │
        └── Arc<Thread> {
              id:              ThreadId             ← immutable; physical execution unit
              session_id:      SessionId            ← immutable; inherited if sub-agent (D9)
              forked_from_id:  Option<ThreadId>     ← immutable; only set by /fork
              parent_thread_id: Option<ThreadId>    ← immutable; only set for sub-agents
              is_root:         bool                 ← derived: parent_thread_id.is_none() — explicit, removes the UUID-comparison footgun (A8)
              state:           Mutex<ThreadState>   ← mutable per-thread config (cwd, permission, …)
              runtime:         Arc<Session>         ← codex-rs Session analog; owns submission_loop
              engine:          Arc<ThreadEngine>    ← coco-rs per-thread engine (D3)
              mcp_connections: McpConnectionsHandle ← per-thread spawn or shared (D4)
              exec_processes:  Arc<UnifiedExecProcessManager> ← own PTY pool
              transcript:      Arc<TranscriptStore> ← writes <thread_id>.jsonl
              active_turn:     Mutex<Option<ActiveTurn>>
              …
            }
```

**Two indexes** in `ThreadManager`:

- `threads: HashMap<ThreadId, Arc<Thread>>` — primary, every inbound message looks up here.
- `sessions: HashMap<SessionId, Vec<Weak<Thread>>>` — secondary, for "all threads in this session"
  queries (Hub session tree, telemetry aggregation, OAuth lookup). `Weak<Thread>` so archiving a
  thread cleans up naturally; periodic GC sweeps dead `Weak` entries.

The legacy `Arc<RwLock<String>> session_id` slot in `session_runtime.rs:474` is **deleted**. It was
neither `ThreadId` nor `SessionId` cleanly — it was a mutable filename handle. Its replacement is
the immutable `Thread.id` (which doubles as the rollout filename).

### 1.7 ID `Display` convention + timestamp accessor

**`Display` is the bare UUID — no prefix, no flags, no metadata.** Timestamp is recoverable from
UUID v7's first 48 bits (ms since epoch), so any "human-readable" rendering decodes on demand and
renders time as a **separate** field — never concatenated into the id string. This avoids
information duplication (the ts would otherwise live both in the prefix and in the uuid's first
12 hex), keeps codex `protocol/src/thread_id.rs:59-63` compatibility byte-for-byte, and leaves
the id stable as more metadata (archive state, sub-agent kind, …) accretes.

```rust
pub struct ThreadId(Uuid);   // same shape for SessionId

impl ThreadId {
    pub fn new() -> Self { Self(Uuid::now_v7()) }

    /// Decode creation time from UUID v7's first 48 bits.
    pub fn created_at(&self) -> SystemTime {
        let ts = self.0.get_timestamp().expect("UUID v7");
        let (secs, nanos) = ts.to_unix();
        SystemTime::UNIX_EPOCH + Duration::new(secs, nanos)
    }
}

impl Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)   // bare uuid — no prefix
    }
}
```

**How renderers compose time + id when they want both** (always as separate fields):

| Surface | id string | time field |
|---|---|---|
| Wire JSON / SDK | bare uuid | not included |
| `tracing` log line | `thread_id=<uuid>` | `created_at=<iso8601>` as a separate span/event field |
| Rollout filename | `<uuid>.jsonl` | `ls -la` shows mtime; alphabetical sort already = chronological sort (UUID v7 property) |
| Hub UI table | id column = uuid | separate `Created` column rendered from `created_at()` |
| Debug message | bare uuid | inline `(created {hh:mm:ss})` only when human-readable context demands it |
| `COCO_SESSION_ID` env | bare uuid | user scripts decode with the SDK helper or `stat` the rollout file |

**Anti-patterns** (do not do):
- `format!("{ts}_{id}")` baked into an id string — duplicates information.
- Encoding `is_root` / `is_archived` / `kind` flags into the id string — couples runtime metadata
  to identifier; bloats parser; breaks if state changes. Keep flags as explicit struct fields on
  `Thread`.
- Truncating uuid to short prefix as a "short id" everywhere — UUID v7 first 12 hex *is* the
  timestamp, so the "random" portion that disambiguates collisions starts at hex char 13. A short
  prefix loses the disambiguator and creates collisions in any second with concurrent thread
  creation.

A single utility helper in `common/utils` may format `"<uuid> (created HH:MM:SS)"` for one-liner
display contexts (errors, CLI summaries) — used *opt-in* by callers, never via the `Display` impl.

## 2. Adversarial-Review Decisions (Resolved)

Eleven decisions (D1–D11) drive the rest of the design; two (D12/D13) are deferred or resolved.
Lock these now or implementation rots.

| # | Decision | Resolution |
|---|---|---|
| D1 | **Identity model** | **Two-level**: `ThreadId` (per-conversation) + `SessionId` (per-session-tree, sub-agent inheritance). NOT a rename. Wire `Thread` carries `id` + `session_id` + `forked_from_id`. Coco's `Arc<RwLock<String>> session_id` slot is deleted; `Thread.id` is immutable. |
| D2 | **Sub-agent / fork identity** | Sub-agents inherit parent's `SessionId`, get fresh `ThreadId`, `forked_from_id = Some(parent.id)`. Forks get fresh `SessionId` (new tree), fresh `ThreadId`, `forked_from_id = Some(source.id)`. Sub-agent and fork operations bypass the `RequestSerializationQueue` — they are intra-turn machinery, not client RPCs. |
| D3 | **`ThreadEngine` extraction** | Mandatory precursor PR. Split `QueryEngine` (today rebuilt per turn) into a per-thread `ThreadEngine` (owns `ArcSwap<ApiClient>`, hooks, tools, command queue) and a per-turn `QueryEngine` view. Without this, multi-thread amplifies the per-turn allocation churn N-fold. |
| D4 | **MCP isolation** | Default **per-thread** (codex parity). Add explicit `mcp.isolation_mode: "per_thread" \| "shared"` field on `thread/create`. Shared mode falls back to a process-wide `McpConnectionManager` with per-thread routing tags. |
| D5 | **Hub connector** | **One connector per process**, multiplexed by `ThreadId` envelope. Threads push into a process-wide `ThreadEventBus` (mpsc fan-in); the connector streams to Hub server. Avoids N WebSocket connections per process. |
| D6 | **`TurnContext`** | **Hybrid snapshot**, not pure snapshot. Snapshot the truly turn-local fields (`model_spec`, `sandbox_policy`, `environments`, `developer_instructions`, `user_instructions`, `network_proxy`, `cwd`). Live (`Arc<RwLock<…>>`) for fields swarm/sub-agent flows need mid-turn (`permission_profile_state`, `approval_policy`, `denial_tracker`). |
| D7 | **`RequestSerializationQueue` scopes** | Ship **3 scopes** initially: `Global(name)`, `Thread { thread_id }`, `McpOauth { server_name }`. Defer codex's 5 other scopes (`Process`, `FsWatch`, `ThreadPath`, `CommandExecProcess`, `FuzzyFileSearchSession`) until concrete callers materialize. |
| D8 | **`/clear` semantics** | `/clear` = `session/create` with `start_source: "clear"` → **brand-new `Thread`** (new `ThreadId`, new `SessionId`, `forked_from_id = None`), old `Thread` shut down, old rollout file preserved on disk for `--resume`. **Identical to `/new` at the identity level**, distinguished only by a hook tag (`SessionStartSource::Clear`) and a TUI terminal-clear step. NOT a fork. `Op::Compact` / `Op::ThreadRollback` stay within the same thread. Mirrors codex `core/src/session/session.rs:515-520` and `tui/src/app/event_dispatch.rs:30-41`. |
| D9 | **Identity model: physical vs logical (re-affirmed)** | Keep **both** `ThreadId` (physical execution unit: one runtime, one rollout, one MCP boundary) and `SessionId` (logical conversation: sub-agent inheritance, telemetry grouping, OAuth scope). Sub-agents have fresh `ThreadId` but inherit parent's `SessionId`. Root threads have `ThreadId.uuid == SessionId.uuid` by `SessionId::from(thread_id)` rule. Earlier "collapse to one id" proposal was withdrawn — it would have broken the parallel-runtime-with-shared-identity contract that sub-agents need. |
| D10 | **End-to-end naming = `session_id`** | Server Rust type `SessionId`, wire field `session_id`, SDK field `session_id`, env var `COCO_SESSION_ID`. **SDK never sees `thread_id`.** `ThreadId` stays server-internal (+ Hub envelope + advanced env var). Diverges from codex SDK (which uses `threadId` for what is actually the root's id) — accept the divergence for semantic clarity. Intermediate name "conversation_id" considered and rejected — three names for one concept is unacceptable. |
| D11 | **Hub envelope carries both ids** | Process-wide `ThreadEventBus` connector frames every event with `{thread_id, session_id, …}`. Hub server keys events by `session_id` for the session-tree view; uses `thread_id` to render per-sub-agent breakdown. SDK clients receiving events filter on `session_id` and treat `thread_id` as opaque. Avoids Hub maintaining a thread-spawn topology state machine; costs ~16 extra bytes per event. |
| D12 | **(deferred)** Type-wall strictness | Whether to delete `impl From<ThreadId> for SessionId` and `impl From<SessionId> for ThreadId` and force `Thread::session_id()` as the only path. Deferred — codex 1:1 port wants `From` retained; revisit if cross-type misuse causes a real bug. |
| D13 | **(resolved no)** Sub-agent breakdown to SDK | Sub-agents stay invisible to SDK. SDK sees one Session; one turn per Session at a time; aggregate cost and events. Operator/Hub UI surfaces breakdown via Hub envelope's `thread_id`. **Trade-off**: SDK users can't directly observe sub-agent cost / progress; counter: keeps the "one conversation = one Session" contract clean. Reverse only if operator-grade SDKs (IDE plugins) need debugging-grade observability — then add via opt-in event subscription, not by surfacing `thread_id`. |

## 3. Architecture

### 3.1 Layered model

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Client                                                                      │
│  ├─ TUI (in-process via InProcessClientHandle)                              │
│  ├─ SDK clients (Python, Node) over stdio                                   │
│  ├─ IDE plugins over WebSocket                                              │
│  └─ Hub web UI via Hub server (which subscribes to one process-wide         │
│      connector, multiplexed by ThreadId)                                    │
├────────────────────────────────────────────────────────────────────────────┤
│ Transport (coco-app-server-transport)                                       │
│  ├─ start_stdio_connection                                                  │
│  ├─ start_unix_socket_acceptor                                              │
│  └─ start_websocket_acceptor                                                │
│      → mpsc<TransportEvent>(128) →                                          │
├────────────────────────────────────────────────────────────────────────────┤
│ App-Server (coco-app-server)                                                │
│  ├─ Inbound Processor task                                                  │
│  │    holds HashMap<ConnectionId, ConnectionState>                          │
│  │    holds Arc<ThreadManager>                                              │
│  │    dispatches:                                                           │
│  │      • no scope: tokio::spawn(handler)                                   │
│  │      • scoped:    RequestSerializationQueue.enqueue(key, request)        │
│  └─ Outbound Router task                                                    │
│        holds HashMap<ConnectionId, OutboundConnectionState>                 │
│        drains outgoing_tx → per-conn writer                                 │
├────────────────────────────────────────────────────────────────────────────┤
│ Thread layer (coco-thread)                                                  │
│  ThreadManager: RwLock<HashMap<ThreadId, Arc<Thread>>>                      │
│   └── Thread (per-conversation, one per active conversation)                │
│        ├── id: ThreadId, session_id: SessionId, forked_from_id              │
│        ├── state: Mutex<ThreadState>                                        │
│        ├── runtime: Arc<Session>          ← Op submission_loop task         │
│        ├── engine:  Arc<ThreadEngine>     ← per-thread engine               │
│        ├── mcp:     McpConnectionsHandle  ← per-thread spawn or shared      │
│        ├── exec:    Arc<UnifiedExecProcessManager>                          │
│        ├── transcript: Arc<TranscriptStore>                                 │
│        └── subscribers: RwLock<HashMap<ConnectionId, mpsc::Sender<Event>>>  │
├────────────────────────────────────────────────────────────────────────────┤
│ Process layer (singletons, Arc-cloned everywhere)                           │
│  AuthManager · RuntimeConfig (watch) · ToolRegistry · CommandRegistry       │
│  McpManager (config registry) · SkillsManager · PluginManager · Hooks       │
│  OutputStyleManager · RoleClientCache · ThreadArchive · ThreadEventBus      │
└────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Crate layout — relationship to existing `app/`

**Q: coco-rs today has no `app-server`. Do we need to introduce one? How does it relate to
`app/cli`, `app/tui`, `app/state`, `app/query`, `app/session`?**

**A: Yes — a new `app/server/` (plus four supporting crates) is the whole point of this plan.**
Today, multi-client / multi-thread concurrency lives nowhere; `app/cli/src/sdk_server/` is a
single-session NDJSON loop on stdio. Each existing `app/` crate has a clear role today and a clear
post-refactor role:

| Existing crate | Role today | Role after refactor |
|---|---|---|
| `app/cli/` | `coco` binary entry; clap; SDK loop (`sdk_server/`); mode dispatch (TUI / SDK / Headless). | Same minus `sdk_server/`. New responsibility: `coco serve --listen ws://… \| unix://… \| stdio` boots the multi-client app-server. `coco sdk` becomes a thin alias to `coco serve --listen stdio`. TUI mode embeds the app-server in-process and connects via `InProcessClientHandle`. |
| `app/tui/` | Ratatui TEA loop. Today consumes `CoreEvent` directly from a process-scoped `QueryEngine` rebuilt per turn. | **Becomes a CLIENT of the app-server** via `app/server-client::InProcessClientHandle`. Sends `thread/create` + `turn/start`, subscribes to `ServerNotification` stream, renders. Stops directly owning `QueryEngine`. |
| `app/session/` | Disk-backed `SessionManager` + transcript JSONL persistence. | **Unchanged in scope.** Relinquishes any runtime / live-state responsibility to `app/thread/`. Each `Thread` holds an `Arc<TranscriptStore>` from this crate; the crate itself stays focused on disk format, archive, resume-read. |
| `app/query/` | `QueryEngine` rebuilt **per turn**; the per-turn agent-loop driver. | **Split per D3:** the per-thread state (hot-swap `ArcSwap<ApiClient>`, hooks, tools, command queue, file-history sink) moves into `app/thread/::ThreadEngine`; `QueryEngine` becomes a thin per-turn view that holds `Arc<ThreadEngine>` + `TurnContext`. The turn-loop logic stays here. |
| `app/state/` | "Swarm" orchestration — sub-agent registry within a session; `Arc<RwLock<AppState>>` tree. | **Lives WITHIN a `Thread`.** Sub-agents allocated via this module follow D2: fresh `ThreadId`, **inherited `SessionId`** (= root's), `forked_from_id = Some(parent.thread_id)`. The `AppState` tree becomes per-`Thread`, not process-wide. |

**5 new crates** under `app/`:

| Crate (new) | Path | Owns |
|---|---|---|
| `coco-app-server-transport` | `app/server-transport/` | `TransportEvent`, `ConnectionId`, three acceptors (`start_stdio_connection`, `start_unix_socket_acceptor`, `start_websocket_acceptor`), per-transport framing (see §3.2.1), `CHANNEL_CAPACITY` constant. Port verbatim from `codex-rs/app-server-transport/src/`. Zero coco-domain types. |
| `coco-app-server-protocol` | `app/server-protocol/` | `JsonRpcMessage` ("JSON-RPC-lite" envelope — `id` / `method` / `params` / `result` / `error` but **no `"jsonrpc":"2.0"` field**, mirroring codex `app-server-protocol/src/jsonrpc_lite.rs:1-3`). `ClientRequest` / `ServerRequest` / `ServerNotification` enums with new thread-aware variants, `ClientRequestSerializationScope` (3 variants), wire `Thread` struct with `id` + `session_id` + `forked_from_id`, `ThreadStartSource { Startup, Clear }` enum (mirrors codex). |
| `coco-app-server` | `app/server/` | `MessageProcessor`, `ThreadManager`, two-task processor pattern, `RequestSerializationQueues`, `ConnectionRpcGate`, per-method handlers (`thread/*`, `turn/*`, `settings/update`, pass-throughs). Binary `bin/coco-app-server`. |
| `coco-app-server-client` | `app/server-client/` | `InProcessClientHandle` (in-memory mpsc channel pair to the embedded `MessageProcessor` — same wire format as remote, zero socket overhead, used by the TUI), `WebSocketClient`, `UdsClient`, request/response correlation, event-stream demux. |
| `coco-thread` | `app/thread/` | The per-`Thread` runtime types: `Thread`, `ThreadState`, `TurnContext`, `ThreadSettings`, `Op`, `ActiveTurn`, `ThreadEngine` (extracted from `app/query` per D3). **Distinct from `app/session/`** — runtime here, persistence there; `Thread` holds an `Arc<TranscriptStore>` but is not it. |

**Post-refactor `app/` has 10 crates in clean dependency layers** (lower never depends on higher):

```
Layer 0  app/server-transport/   ← WS / UDS / Stdio acceptors; pure I/O
Layer 1  app/server-protocol/    ← JsonRpcMessage; wire enums; no logic
Layer 2  app/session/            ← Disk transcript persistence (unchanged role)
Layer 3  app/thread/             ← Thread, TurnContext, ThreadEngine, Op
         app/query/              ← Per-turn QueryEngine view
         app/state/              ← Sub-agent / swarm orchestration WITHIN a Thread
Layer 4  app/server/             ← MessageProcessor + ThreadManager + dispatch
Layer 5  app/server-client/      ← InProcessClientHandle / WebSocketClient / UdsClient
         app/tui/                ← Ratatui client of app-server (via InProcessClientHandle)
Layer 6  app/cli/                ← Binary entry; bootstraps ProcessRuntime; mode dispatch
```

The TUI is at Layer 5 because it is a client of the app-server, not a peer. The CLI is at Layer 6
because it boots everything. The split between `app/server-client/` (Layer 5) and `app/server/`
(Layer 4) is what lets the TUI talk over an in-memory `InProcessClientHandle` (zero overhead) while
external clients (Python SDK, IDE plugins) use the same wire protocol over WS / UDS / stdio.

`app/cli/src/sdk_server/` (the whole tree: `handlers/`, `transport.rs`, `dispatcher.rs`,
`outbound.rs`, `pending_map.rs`, `sdk_runner.rs`) is **deleted wholesale** in PR 11.

### 3.2.1 Wire framing vs envelope (orthogonal concerns)

**Framing** (how bytes split into messages) and **envelope** (what fields the message contains) are
two separate layers. The receiver/decoder pipeline composes them. Confusing the two has cost time
in prior planning — keep them mentally apart.

| Layer | Answers | Examples |
|---|---|---|
| **Framing** | "Where does one message end and the next start?" | NDJSON (split on `\n`) · WebSocket frames · HTTP chunked · length-prefix · ASCII null-terminated |
| **Envelope (protocol)** | "What fields are inside one message?" | JSON-RPC 2.0 (`id`/`method`/`params`/`result`/`error`/`"jsonrpc":"2.0"`) · gRPC · REST · raw JSON |

coco-rs mirrors codex's decision: **same envelope on every transport, different framing per
transport**. The Transport layer normalises all three into one uniform
`TransportEvent::IncomingMessage { connection_id, message: JsonRpcMessage }`, so the upstream
`MessageProcessor` doesn't know (or care) which transport delivered the bytes.

| Transport | Framing | Envelope | Source-of-truth in codex |
|---|---|---|---|
| **stdio** | **NDJSON** — `BufReader.lines()`, one JSON object per `\n`-terminated line | JSON-RPC-lite | `codex-rs/app-server-transport/src/transport/stdio.rs:45-46` |
| **UDS** (Unix Domain Socket) | **WebSocket** — `tokio_tungstenite::accept_async(unix_stream)`, then text frames. **Not NDJSON.** | JSON-RPC-lite | `codex-rs/app-server-transport/src/transport/unix_socket.rs:7,79` (`use websocket::run_websocket_connection` + `accept_async`) |
| **WebSocket (TCP)** | **WebSocket** — text frames | JSON-RPC-lite | `codex-rs/app-server-transport/src/transport/websocket.rs:259,350` |

**Why UDS reuses WebSocket framing** (and we should too):

1. Don't write a second framer. WS is already in the codebase for TCP-WS; reusing it on a Unix
   stream is a one-line `accept_async(unix_stream)` change.
2. Client-side code is ~99% shared between TCP-WS and UDS — only the `Stream` type differs.
3. WS gives ping/pong / close-frame semantics for free → dead-connection detection on local sockets.
4. Binary extension space (msgpack / protobuf) doesn't require swapping transports.

**Why stdio stays NDJSON** (not WS):

1. Stdio is a half-duplex byte stream from a parent process. WS handshake on stdio adds latency and
   complexity for no gain — there's no multiplexing, no out-of-band control frames worth having.
2. Existing Python/Node SDK clients that pipe to/from a `coco sdk` subprocess already speak NDJSON.
3. One connection per process; framing complexity buys nothing.

**JSON-RPC-lite, not strict JSON-RPC 2.0.** Per `codex-rs/app-server-protocol/src/jsonrpc_lite.rs:1-3`:

```rust
//! We do not do true JSON-RPC 2.0, as we neither send nor expect the
//! "jsonrpc": "2.0" field.
```

We do the same. Saves 16 bytes per message and clients don't actually depend on the version field.
The shape (`id`/`method`/`params`/`result`/`error`) is otherwise identical to JSON-RPC 2.0.

### 3.2.2 How each crate enforces `ThreadId` physical isolation

`ThreadId` is the physical execution unit. Real isolation only works if every piece of per-thread
state lives **inside** the `Thread` and **never** leaks across `ThreadId` boundaries. Layer
discipline must enforce this — `Arc<Thread>` is the only path to per-thread state.

| Layer / crate | Per-thread asset | Enforcement mechanism |
|---|---|---|
| `app/thread/` (L3) | `Arc<Session>` runtime (single `submission_loop` tokio task) | One task per `Thread::new()`; `JoinHandle` stored on the `Thread`. Dropping the `Arc<Thread>` aborts the task. **No code outside `app/thread/` can construct a `Session`.** |
| `app/thread/` (L3) | `active_turn: Mutex<Option<ActiveTurn>>` | The mutex is on the `Thread` struct itself. Cross-thread access requires holding `Arc<Thread>`, which goes through `ThreadManager::get(thread_id)`. |
| `app/session/` (L2) | `Arc<TranscriptStore>` writing `<thread_id>.jsonl` | Filename derived from `Thread.id` at construction time and **immutable**. Two threads cannot accidentally share a rollout file. |
| `services/mcp/` (L3) | `McpConnectionsHandle` per thread (per D4 `per_thread` mode) | Each `Thread` owns its own `Arc<RwLock<McpConnectionManager>>`; the manager spawns its own child processes. Killing thread A's MCP child has zero blast radius on thread B's. **`COCO_THREAD_ID` env var injected into spawn** — child process self-attributes. |
| `exec/exec-server/` (L3) | `Arc<UnifiedExecProcessManager>` PTY pool | Per-thread instance; PTY IDs allocated within the manager (no global pool). Closing a Thread reclaims all its PTYs. |
| `exec/shell/` (L3) | Per-command spawn | `Command` env contains `COCO_THREAD_ID` + `COCO_SESSION_ID` from `TurnContext.thread_id` / `.session_id`. Spawn-time only — no runtime crossover. |
| `app/query/` (L3) | `QueryEngine` (per-turn view over per-thread `ThreadEngine`) | `ThreadEngine` is owned by the `Thread`; `QueryEngine` borrows it for the turn lifetime, dropped at turn end. No singleton, no global cache. |
| `app/state/` (L3) | Sub-agent registry within a `Thread` | `AppState` tree is per-`Thread`. Sub-agents register into their parent's `app/state` (via `Arc<Thread>` back-ref through `Weak<Thread>` to avoid cycles). Sub-agent failure can't corrupt sibling thread state because there's no sibling-shared state. |
| `app/server/` (L4) | `ThreadManager: RwLock<HashMap<ThreadId, Arc<Thread>>>` | Single source of truth for "which threads exist". Inbound requests resolve `thread_id` → `Arc<Thread>` here; subsequent processing operates **only** through the `Arc<Thread>`. No `&'static` thread caches, no thread-local pools. |
| `app/server/` (L4) | `RequestSerializationQueues` per `Thread { thread_id }` scope | Mutating requests (`turn/start`, `settings/update`, `mcp/setServers`) serialize per-`ThreadId`. Concurrent inbound requests on the same thread cannot interleave their state mutations. |
| `app/server-transport/` (L0) | Connection-to-thread routing | `ConnectionId` is independent of `ThreadId`. One connection can drive multiple threads; one thread can be observed by multiple connections. Cross-`ConnectionId` interrupts (Conn 2 interrupts thread A driven by Conn 1) work by construction. |
| `hub/connector/` (Standalone) | One bus per process, framed by `(thread_id, session_id)` per D5/D11 | Per-thread fan-in via `ThreadEventBus`; no per-thread WebSocket connection to Hub. Connector is the only place where cross-thread events meet — but only for read/observe. |

**Anti-patterns that break isolation** (must be caught in code review):

- `lazy_static!` / `OnceCell` holding any per-thread state. Acceptable only for truly process-wide
  registries (tool catalog, command catalog).
- A tool implementation reaching into `&AppState` for "the current session" — there is no current
  session. Tools receive `&TurnContext` which gives access to **their** `Thread`.
- Sharing a `mpsc::Sender<CoreEvent>` across threads. Each `Thread` has its own
  `event_tx: mpsc::Sender<CoreEvent>` and its own subscriber registry.
- Background tasks (`tasks/` crate) without a `thread_id` tag — every spawned task carries the
  `ThreadId` of its originating thread; lookup, cancel, and event routing key on it.
- File-system caches keyed by global path. If a cache is per-thread, key the cache map by
  `(ThreadId, path)`.

The layered crate graph (§3.2) makes most of these mistakes type-impossible: e.g. `exec/shell`
can't import `app/server`, so a shell command implementer cannot "look up the current thread" —
they must receive a `TurnContext` from the runtime.

### 3.3 Type definitions

#### Process-wide

One `Arc<ProcessRuntime>`, cloned cheaply into every `Thread`. Built once at startup.

```rust
pub struct ProcessRuntime {
    pub auth_manager: Arc<AuthManager>,              // future-proofed for multi-account via wrapper
    pub config_home: AbsolutePathBuf,
    pub runtime_config_rx: watch::Receiver<Arc<RuntimeConfig>>,
    pub mcp_manager: Arc<McpManager>,                // config registry ONLY (§3.6)
    pub skills_manager: Arc<SkillsManager>,
    pub plugin_manager: Arc<PluginManager>,
    pub command_registry: Arc<RwLock<Arc<CommandRegistry>>>,
    pub tool_registry: Arc<ToolRegistry>,
    pub hook_registry: Arc<HookRegistry>,
    pub output_style_manager: Arc<OutputStyleManager>,
    pub role_client_cache: Arc<RoleClientCache>,
    pub installation_id: String,
    pub thread_archive: Arc<TranscriptArchive>,      // disk-backed; powers thread/resume
    pub thread_event_bus: Arc<ThreadEventBus>,       // mpsc fan-in for Hub connector (D5)
}
```

#### Per-thread

One `Arc<Thread>` per active conversation, keyed by `ThreadId` in `ThreadManager`.

```rust
pub struct Thread {
    pub id: ThreadId,                                // immutable
    pub session_id: SessionId,                       // immutable; inherited if sub-agent
    pub forked_from_id: Option<ThreadId>,            // immutable; audit
    pub source: ThreadSource,                        // Cli | Sdk | Tui | Hub | SubAgent { parent: ThreadId }
    pub created_at: Instant,
    pub process: Arc<ProcessRuntime>,                // Arc-clone

    pub state: tokio::sync::Mutex<ThreadState>,      // short critical sections
    pub runtime: Arc<Session>,                       // codex-rs Session analog
    pub engine: Arc<ThreadEngine>,                   // per D3
    pub mcp_connections: McpConnectionsHandle,       // per D4
    pub exec_processes: Arc<UnifiedExecProcessManager>,
    pub transcript: Arc<TranscriptStore>,            // writes <thread_id>.jsonl
    pub history: Arc<tokio::sync::Mutex<MessageHistory>>,

    pub active_turn: tokio::sync::Mutex<Option<ActiveTurn>>,
    pub turn_cancel: tokio::sync::Mutex<Option<CancellationToken>>,
    pub command_queue: CommandQueue,

    pub subscribers: RwLock<HashMap<ConnectionId, mpsc::Sender<ServerNotification>>>,
    pub submission_tx: mpsc::Sender<Op>,
    pub _loop_handle: JoinHandle<()>,                // submission loop, joined on archive
}

pub struct ThreadState {
    pub configuration: SessionConfiguration,         // cwd, permission_profile, model, instructions, environments
    pub thread_name: Option<String>,
    pub features: ManagedFeatures,
    pub mcp_isolation_mode: McpIsolationMode,
    pub pending_mcp_refresh: Option<McpServerRefreshConfig>,
}

pub enum McpConnectionsHandle {
    PerThread(Arc<RwLock<McpConnectionManager>>),    // owns this thread's child processes
    Shared(Arc<RwLock<McpConnectionManager>>),       // pointer to process-wide instance
}

pub enum ThreadSource {
    Cli,
    Sdk { client_name: Option<String> },
    Tui,
    Hub,
    SubAgent { parent_thread_id: ThreadId, parent_session_id: SessionId },
}
```

`ThreadSource::SubAgent` carries `parent_session_id` so allocation rules in §1.3 can fire without
reading parent state.

#### Per-turn

`TurnContext` is **hybrid** (per D6) — built fresh in the submission loop, dropped at turn end:

```rust
pub struct TurnContext {
    pub thread_id: ThreadId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub trace_id: Option<String>,

    // Snapshot (immutable for the turn)
    pub model_spec: ModelSpec,
    pub sandbox_policy: SandboxPolicy,
    pub environments: ResolvedTurnEnvironments,
    pub developer_instructions: Option<String>,
    pub user_instructions: Option<String>,
    pub network_proxy: Option<NetworkProxy>,
    pub cwd: AbsolutePathBuf,

    // Live (shared with ThreadState, mid-turn observable)
    pub permission_profile_state: Arc<RwLock<PermissionProfileState>>,
    pub approval_policy: Arc<RwLock<AskForApproval>>,
    pub denial_tracker: Arc<Mutex<DenialTracker>>,

    // Refs
    pub config: Arc<Config>,
    pub client: Arc<ApiClient>,                      // snapshot of Arc — hot-swap won't yank
    pub thread: Weak<Thread>,                        // back-reference, weak to avoid cycles
}
```

### 3.4 Dispatch architecture

Two-task pattern, copied from `codex-rs/app-server/src/lib.rs:733-1060`. No invention.

```text
[Stdio  →  NDJSON framer    ]─┐
[UDS    →  WS framer (over   ]─┼─→ transport_event_tx(128) ─→ [Inbound Processor]
[          unix stream)      ] │       (all decoded to JsonRpcMessage by now)
[TCP-WS →  WS framer         ]─┘                                  │
                                                                  │ HashMap<ConnectionId, ConnectionState>
                                                                  │ Arc<ThreadManager>
                                                                  │
                                                       dispatch ──┤
                                                            │     │
                                                no scope ───┼── tokio::spawn(request.run())     [concurrent]
                                                            │
                                                has scope ──┴── request_serialization_queues
                                                                .enqueue(key, access, request)
                                                                └── per-key drain task ── request.run()

[All request.run() bodies] ──→ outgoing_tx(128) ──→ [Outbound Router]
                                                         │ HashMap<ConnectionId, OutboundConnectionState>
                                                         │ routes envelope.connection_id → per-conn writer
                                                         └─→ writer re-frames (NDJSON for stdio, WS for UDS/TCP-WS)
                                                              → bytes on wire
```

**Framing is encapsulated in the acceptor / writer pair per transport.** The Processor only ever
sees decoded `JsonRpcMessage`; the Outbound Router only ever emits decoded `JsonRpcMessage`. Each
transport's writer task does its own re-framing (NDJSON appends `\n`; WS wraps in a text frame).

**Channel capacities:** `transport_event_tx: mpsc(128)` (codex parity; overflow returns JSON-RPC
error `-32001` "Server overloaded"). `outgoing_tx: mpsc(128)`. Per-connection writer: `mpsc(256)`
for WS / UDS (high streaming-delta burst), `mpsc(128)` for stdio. Per-thread `submission_tx: mpsc(64)`
(turns are user-paced). Per-thread `event_tx: mpsc(512)` (stream deltas burst).

Channel capacities are config knobs (`runtime.transport_channel_capacity`) for future tuning. Single
processor task is acceptable at coco-rs's expected scale (≤30 threads, ≤2 in-flight RPCs/thread);
sharding deferred until measured contention.

### 3.5 Serialization scopes (3 variants per D7)

```rust
pub enum ClientRequestSerializationScope {
    Global { name: &'static str, access: ScopeAccess },
    Thread { thread_id: ThreadId, access: ScopeAccess },
    McpOauth { server_name: String },
}
pub enum ScopeAccess { Exclusive, SharedRead }
```

| Request | Scope | Why |
|---|---|---|
| `thread/create` | `Global("thread_manager") Exclusive` | Inserts into registry. |
| `thread/list` | `Global("thread_manager") SharedRead` | Snapshot read; many readers OK. |
| `thread/archive` | `Thread { thread_id } Exclusive` | Drains submission loop. |
| `thread/resume` | `Global("thread_manager") Exclusive` | Insert + duplicate-resume rejection. |
| `thread/fork` | `Global("thread_manager") Exclusive` | Allocates new `ThreadId` + new `SessionId`. |
| `turn/start` | `Thread { thread_id } Exclusive` | Mutates `ThreadState`, installs `ActiveTurn`. |
| `turn/interrupt` | none (`tokio::spawn` direct) | Atomic cancel signal; must never block. |
| `settings/update` | `Thread { thread_id } Exclusive` | Patches `ThreadState.configuration`. |
| `mcp/setServers` | `Thread { thread_id } Exclusive` | Rebuilds per-thread `McpConnectionManager`. |
| `mcp/oauth/start` | `McpOauth { server_name } Exclusive` | OAuth flows can't overlap per server. |
| `config/read` | `Global("config") SharedRead` | Many concurrent readers OK. |
| `config/write` | `Global("config") Exclusive` | Broadcasts via `watch` channel. |
| `fs/*` | none | Concurrent filesystem ops fine. |

**Reentrancy:** handler running under `Thread { thread_id }` Exclusive **must not** make a nested
client RPC that would re-enqueue on the same key. Internal subagent / fork dispatches bypass the
queue (per D2), so this is safe by construction. Tests in PR 12 cover this.

### 3.6 MCP per-thread isolation

Replace today's single pool (`app/cli/src/sdk_server/handlers/mod.rs:306 mcp_manager`) with two
layers:

- **`McpManager` (process-wide, config registry only).** Owns config snapshot, OAuth token store,
  config-file watcher. Does not spawn any MCP server processes.
- **`McpConnectionManager` (per-thread or shared per D4).** Actually spawns / connects. For
  `McpIsolationMode::PerThread`, each `Thread::new()` constructs its own from
  `process.mcp_manager.configs.read()`. For `Shared`, all threads point at one process-wide instance.

`mcp/setServers` mid-thread runs under `Thread { thread_id } Exclusive`. The handler diffs against
the per-thread registry, disconnects removed servers (this thread's children), spawns added servers
(this thread's new children), emits `McpServersChanged { thread_id }`. Process-wide config edits
(`config/write` for `~/.coco/mcp_servers.json`) fire the watcher; each `Thread` queues a refresh in
`state.pending_mcp_refresh` that runs on next idle.

### 3.7 `COCO_SESSION_ID` and `COCO_THREAD_ID` injection

Add to `common/types/src/shell_environment.rs`:

```rust
pub const COCO_SESSION_ID_ENV_VAR: &str = "COCO_SESSION_ID";   // logical — DEFAULT for hooks
pub const COCO_THREAD_ID_ENV_VAR:  &str = "COCO_THREAD_ID";    // physical — advanced telemetry
```

Inject **both** into every spawned-child env at:

- `exec/shell/src/runtime.rs` — bash command spawn (merge into `Command` env).
- `exec/exec-server/src/manager.rs` — unified-exec PTY spawn.
- `services/mcp/src/client.rs::do_connect` — stdio MCP child spawn (via
  `ScopedMcpServerConfig.runtime_env`).

**Which one to read** (documented in hooks doc):

| Reader | Default | Why |
|---|---|---|
| User hook scripts | **`COCO_SESSION_ID`** | "This conversation" — aggregates across all sub-agents in one root. Matches user mental model. |
| Hub web UI (per-process log attribution) | both | `session_id` for the tree view; `thread_id` for per-sub-agent breakdown. |
| Telemetry exporters | both | Default dimension `session_id`; `thread_id` available for sub-agent drilldown. |
| Sub-subprocesses (recursive spawns) | inherit both (env passes naturally) | Free attribution. |

The asymmetry: **`COCO_SESSION_ID` is the public-facing identifier** (what hooks should
default to), `COCO_THREAD_ID` is advanced. This mirrors the SDK/wire choice (D10): logical id is
the public contract; physical id is the internal detail.

## 4. Wire Protocol

JSON-RPC-lite envelope (§3.2.1) in `coco-app-server-protocol::JsonRpcMessage`. **All SDK-facing
method names use the `session/*` namespace** — never `thread/*`. `SessionId` is the only ID in
`params` and event payloads on the SDK surface (D10).

### 4.1 SDK ↔ app-server methods (`session/*` namespace)

```jsonc
// session/create  — used by /new and /clear; only start_source differs
{ "type":"request", "id":1, "method":"session/create",
  "params":{
    "name":"my-task",
    "cwd":"/abs/path",
    "model":"anthropic/claude-opus-4-7",
    "permission_mode":"auto",
    "start_source":"startup",                    // or "clear" for /clear
    "mcp":{ "isolation_mode":"per_thread" },
    "settings_overrides":{ /* SessionSettings */ }
  }
}
// → { "type":"response", "id":1, "result":{
//      "session": {
//        "id":"01HXY…",                         // SessionId — the only id SDK sees
//        "is_root": true,
//        "forked_from_session_id": null,
//        "cwd":"/abs/path", "model":"…", "created_at":"2026-…"
//      }
//    } }
//
// /clear flow at the TUI / SDK layer:
//   1. (TUI only) clear terminal UI
//   2. session/archive { session_id: <old> }            ← shutdown old runtime
//   3. session/create  { start_source: "clear", … }     ← spawn fresh runtime
//   4. (TUI only) hint "To resume, run `coco --resume <old_session_id>`"
// The old session's rollout JSONL stays on disk.

// session/fork  — distinct from /clear; copies a snapshot of parent history
{ "type":"request", "id":2, "method":"session/fork",
  "params":{
    "source_session_id":"01HXY…",
    "snapshot":{ "kind":"truncate_before_nth_user_message", "n":3 }
  }
}
// → fresh SessionId + forked_from_session_id = Some("01HXY…")
//   history = truncated copy of parent's first 3 user messages

// session/list
{ "type":"request", "id":3, "method":"session/list", "params":{} }
// → { "result": { "sessions": [ {SessionSummary}, … ] } }

// session/archive
{ "type":"request", "id":4, "method":"session/archive",
  "params":{ "session_id":"01HXY…" }
}

// session/resume
{ "type":"request", "id":5, "method":"session/resume",
  "params":{ "session_id":"01HXY…" }
}

// turn/start
{ "type":"request", "id":6, "method":"turn/start",
  "params":{
    "session_id":"01HXY…",
    "input":[ { /* UserInputItem */ } ],
    "session_settings":{ "cwd":"/other/path" }    // per-turn overrides
  }
}

// turn/interrupt
{ "type":"request", "id":7, "method":"turn/interrupt",
  "params":{ "session_id":"01HXY…" }
}

// settings/update
{ "type":"request", "id":8, "method":"settings/update",
  "params":{ "session_id":"01HXY…", "updates":{ /* SessionSettingsOverrides */ } }
}
```

`ServerNotification` variants tagged with **`session_id` only** on SDK-facing events:
`TurnStarted`, `TurnCompleted`, `TextDelta`, `ToolUseStarted`, `McpDisconnected`,
`SessionSettingsApplied`, …

### 4.2 Hub envelope — both ids (D11)

Events flowing through `hub/connector/` → Hub server use a richer envelope:

```jsonc
// One bus event, framed by both ids
{
  "session_id": "01HXY…",                // logical: tree grouping in Hub UI
  "thread_id":  "01HXZ…",                // physical: which sub-agent runtime emitted this
  "is_root_thread": false,               // false → sub-agent rendering
  "ts": "2026-05-26T…",
  "payload": { /* CoreEvent */ }
}
```

The Hub UI groups by `session_id` to render the session tree; uses `thread_id` to label per-sub-agent
breakdown rows. SDK clients receive only the `session_id`-framed view (Hub envelope is internal to
the connector ↔ Hub-server boundary).

### 4.3 Server internals (NOT exposed)

Internal Rust APIs operate on `Arc<Thread>` (keyed by `ThreadId`). The mapping
`session_id → Vec<Arc<Thread>>` lives in `ThreadManager.sessions` (§1.6). Sub-agent spawn flows
internally allocate `ThreadId` + `parent_thread_id`; the new thread inherits `session_id` from
parent. None of this is visible at the SDK wire boundary.

## 5. Per-Thread vs Per-Turn Settings Flow

Per codex-rs `Op::UserInput { thread_settings }` pattern (`core/src/session/handlers.rs:192`):

```rust
pub enum Op {
    UserInput {
        items: Vec<UserInputItem>,
        environments: Option<Vec<TurnEnvironmentSelection>>,
        thread_settings: ThreadSettingsOverrides,    // empty = carry-forward
        final_output_json_schema: Option<serde_json::Value>,
    },
    ThreadSettings { updates: ThreadSettingsOverrides },   // change settings without a turn
    Interrupt,
    UserInputAnswer { id: String, response: serde_json::Value },
    Compact,                                                // shrink history in place
    ThreadRollback { num_turns: u32 },                      // drop last N from memory
}
```

Per-thread submission loop (one tokio task per `Thread`, spawned in `thread/create`):

```rust
while let Some(op) = submission_rx.recv().await {
    match op {
        Op::UserInput { thread_settings, items, environments, final_output_json_schema } => {
            if !thread_settings.is_empty() {
                let applied = apply_thread_settings(&self, thread_settings).await;
                emit_thread_settings_applied(&self, applied).await;
            }
            let turn_ctx = TurnContext::snapshot_from(&self).await;
            self.engine.run_turn(turn_ctx, items, environments).await;
        }
        Op::ThreadSettings { updates } => { /* apply, emit, no turn */ }
        Op::Interrupt => { /* cancel active_turn */ }
        Op::UserInputAnswer { .. } => { /* feed to pending_user_input */ }
        Op::Compact => { /* trigger in-place compaction; same ThreadId */ }
        Op::ThreadRollback { num_turns } => { /* drop last N from memory; same ThreadId */ }
    }
}
```

`ThreadSettingsOverrides` is `Option<T>`-shaped for every settable field (`cwd`, `permission_profile_id`,
`model`, `approval_policy`, `personality`, `developer_instructions`, …); `default()` = all `None`
= carry-forward.

## 6. Sharing Decisions (Recap)

| Resource | Scope | Type |
|---|---|---|
| `AuthManager` | Process | `Arc<AuthManager>` (future: `Arc<AuthRegistry>` for multi-account) |
| `RuntimeConfig` | Process, hot-reload | `watch::Receiver<Arc<RuntimeConfig>>` |
| `ToolRegistry` | Process, immutable | `Arc<ToolRegistry>` |
| `CommandRegistry` | Process, hot-reload | `Arc<RwLock<Arc<CommandRegistry>>>` |
| `SkillsManager` / `PluginManager` / `HookRegistry` / `OutputStyleManager` | Process | `Arc<…>` |
| `McpManager` (config) | Process | `Arc<McpManager>` |
| `McpConnectionManager` (connections) | Per-thread (default) / Shared | `McpConnectionsHandle` |
| `UnifiedExecProcessManager` | Per-thread | `Arc<UnifiedExecProcessManager>` |
| `ThreadEngine` | Per-thread | `Arc<ThreadEngine>` (per D3) |
| `ApiClient` (active role) | Per-thread, hot-swap | `ArcSwap<ApiClient>` inside `ThreadEngine` |
| `MessageHistory` | Per-thread | `Arc<tokio::sync::Mutex<MessageHistory>>` |
| `Thread` registry (physical) | Process | `RwLock<HashMap<ThreadId, Arc<Thread>>>` in `ThreadManager` — primary lookup |
| Session index (logical) | Process | `RwLock<HashMap<SessionId, Vec<Weak<Thread>>>>` in `ThreadManager` — for Hub session tree, telemetry aggregation, OAuth lookup. `Weak` so archived threads GC naturally. |
| `TurnContext` | Per-turn | Constructed per `Op::UserInput`, dropped at turn end |
| `permission_profile_state` / `approval_policy` | Per-thread, live (D6) | Shared `Arc<RwLock<…>>` between `ThreadState` and `TurnContext` |

## 7. Migration Plan — Ordered, Atomic PRs

Each PR compiles + tests cleanly before the next. No big-bang rewrite. PRs are independent enough to
be reviewed separately.

| PR | Title | What it does |
|---|---|---|
| 1 | **Foundation types** | Add `ThreadId`, `SessionId`, `ConnectionId` newtypes to `common/types` (mirroring codex shape, with `From` conversions between `ThreadId` and `SessionId`). Tests cover round-trip serde. **No behavior change yet.** |
| 2 | **Delete `Arc<RwLock<String>> session_id` from session_runtime** | Mechanical: remove the rotating string slot; introduce a typed `transcript_handle: TranscriptHandle` for the (still-mutable) on-disk filename. Audit every `session_id.read()` / `.write()` call site. |
| 3 | **`ThreadEngine` extraction** (per D3) | Split `app/query::QueryEngine`: extract `ThreadEngine` (per-thread, owns hot-swap `ArcSwap<ApiClient>`, hooks, tools, command queue, file history) + `QueryEngine` becomes a per-turn view. Validate in the still-single-session world. **This is the critical perf precursor; not optional.** |
| 4 | **`coco-app-server-transport`** | Port from `codex-rs/app-server-transport/` verbatim: `TransportEvent`, `ConnectionId`, three acceptors. Extract NDJSON framing from existing `sdk_server::StdioTransport`. Port codex's `transport_tests.rs`. |
| 5 | **`coco-app-server-protocol`** | Move `JsonRpcMessage` from `coco-types`. Add new request enums (`ThreadCreate`, `ThreadFork`, `ThreadResume`, `TurnStart`, `TurnInterrupt`, `SettingsUpdate`, …). Wire `Thread` struct with `id` + `session_id` + `forked_from_id`. `ClientRequestSerializationScope` (3 variants). |
| 6 | **`ProcessRuntime` / `Thread` split** | Refactor `SessionRuntime`: split into `ProcessRuntime` (immutable post-startup or hot-reload) + per-thread `Thread`. `ThreadManager` (`RwLock<HashMap<ThreadId, Arc<Thread>>>`). Per-thread submission loop spawned on `thread/create`. `TurnContext` (hybrid per D6) constructed in submission loop. **Largest PR — touches every call site that today reaches into `SessionRuntime`.** |
| 7 | **MCP per-thread split** | `McpManager` (process registry) vs `McpConnectionManager` (per-thread or shared per `McpIsolationMode`). `thread/create` accepts `mcp.isolation_mode`. |
| 8 | **`COCO_THREAD_ID` + `COCO_SESSION_ID` injection** | `exec/shell`, `exec/exec-server`, `services/mcp` all accept identity from `TurnContext` and inject into child env. |
| 9 | **`coco-app-server`** | `MessageProcessor`, `RequestSerializationQueues` (3 scopes), outbound router task, inbound processor task. Per-method handlers for `thread/*`, `turn/*`, `settings/update`, plus pass-throughs (`fs/*`, `config/*`, `mcp/*`). |
| 10 | **`coco-app-server-client`** | `InProcessClientHandle` (in-process Arc + in-memory mpsc pair to the same `MessageProcessor` — same wire format as remote, no socket overhead). `WebSocketClient`, `UdsClient` dialers. |
| 11 | **CLI rewire + Hub connector multiplex** | New `coco serve --listen ws://… \| unix://… \| stdio`. `coco sdk` aliased to `coco serve --listen stdio`. TUI uses `InProcessClientHandle`. `hub/connector/` refactored to 1-per-process multiplexing `ThreadEventBus` (per D5). **Delete `app/cli/src/sdk_server/` wholesale.** |
| 12 | **SDK regen — minimal refactor** | `coco-sdk/python/src/coco_sdk/` already uses `session_*` naming throughout (`SessionNotFoundError`, `client.read_session(session_id)`, `_session_id`, `session/resume` wire method) — **no big rename needed**. Per D10/D13, SDK keeps `session_id` end-to-end and never sees `thread_id`. Work scope: (a) run `coco-sdk/scripts/generate_all.sh` to regen `_internal/generated/protocol.py` from the new `coco-app-server-protocol` schemas; (b) bump wire method strings if the server adopts a v2 namespace (`session/create` may stay `session/start` for codex parity — confirm before this PR); (c) audit `examples/` and `tests/` for any literal `session/start` → updated method name. Estimated 10-20 manual call sites + 1 regen. Document the wire-method bump (not an API surface change) in release notes. |
| 13 | **Tests, docs, CLAUDE.md regen** | Integration tests (§8), load test, regenerate CLAUDE.md for new crates, update root `CLAUDE.md` architecture diagram. |

## 8. Verification

### 8.1 Unit tests

- `app/server/src/request_serialization.test.rs` — port codex's full suite verbatim:
  `same_key_requests_run_fifo`, `different_keys_run_concurrently`,
  `closed_gate_request_is_skipped_and_following_requests_continue`,
  `same_key_shared_reads_run_concurrently`,
  `exclusive_write_waits_for_running_shared_reads`,
  `later_shared_reads_do_not_jump_ahead_of_queued_write` (codex `request_serialization.rs:204-682`).
- `app/server-transport/src/transport.test.rs` — port
  `enqueue_incoming_request_returns_overload_error_when_queue_is_full`.
- `app/thread/src/thread_manager.test.rs` — 100 concurrent `ThreadManager::create` → 100 distinct
  `ThreadId`s, no duplicates, no panics. Sub-agent inheritance: spawn 10 sub-agents under root,
  assert all 11 share one `SessionId`. Fork: assert child `SessionId != parent.session_id`.
- `app/thread/src/turn_context.test.rs` — verify snapshot fields immutable mid-turn; verify live
  fields (`permission_profile_state`) observable to mid-turn observers (D6 round-trip).

### 8.2 Integration tests (`tests/` at workspace root)

- **`multi_client_isolation.rs`** — Spawn `coco serve --listen unix:///tmp/test.sock`. Two
  stdio-to-uds clients. Each calls `thread/create` (different cwd, different model). Concurrent
  `turn/start` with marker prompts (`echo "<<A>>"`, `echo "<<B>>"`). Assert: 2 distinct `ThreadId`s
  and 2 distinct `SessionId`s, markers don't leak between threads, per-thread transcript files
  contain only their own marker.
- **`subagent_session_inheritance.rs`** — Create root thread R. Run turn that spawns sub-agent S via
  Agent tool. Assert: `R.session_id == S.session_id`, `R.id != S.id`, `S.forked_from_id == Some(R.id)`.
  Bash tool inside S sees `COCO_SESSION_ID == R.session_id`, `COCO_THREAD_ID == S.id`.
- **`clear_starts_fresh_thread.rs`** — Create root thread R, run 2 turns. Simulate `/clear`:
  `thread/archive { thread_id: R }` then `thread/create { start_source: "clear" }` → new thread C.
  Assert: `C.id != R.id`, `C.session_id != R.session_id`, `C.forked_from_id == None`,
  C's history is empty, R's rollout JSONL still on disk and readable by `thread/resume`.
  Verifies that `/clear` is identity-fresh, NOT a fork.
- **`fork_preserves_lineage.rs`** — Create root thread R, run 2 turns.
  Call `thread/fork { source_thread_id: R, snapshot: { kind: "truncate_before_nth_user_message", n: 1 } }`
  → child F. Assert: `F.id != R.id`, `F.session_id != R.session_id` (new session tree),
  `F.forked_from_id == Some(R.id)`, F's history contains R's first user message only.
  Together with `clear_starts_fresh_thread.rs`, this nails the `/clear` vs `/fork` distinction.
- **`mcp_per_thread.rs`** — Configure stub MCP server that prints `$COCO_THREAD_ID` and
  `$COCO_SESSION_ID` on startup. Two threads with `mcp.isolation_mode: "per_thread"`. Assert each
  MCP child has the correct env (verified via `/proc/<pid>/environ`). Kill thread A's MCP process.
  Assert thread B's MCP still responsive; thread A emits `McpDisconnected { thread_id: A }`.
- **`mcp_shared_mode.rs`** — Two threads with `mcp.isolation_mode: "shared"`. Assert single MCP
  process for both threads. Per-thread routing tag visible in MCP request payloads.
- **`settings_update_mid_thread.rs`** — Create thread, run turn 1 in `cwd=/tmp/a`. Send
  `settings/update { cwd: /tmp/b }`. Assert `ThreadSettingsApplied` event. Run turn 2 with `pwd`
  tool. Assert tool sees `/tmp/b`.
- **`cross_connection_interrupt.rs`** — Conn 1 starts long turn on thread A. Conn 2 (different
  stdio client, same UDS) sends `turn/interrupt { thread_id: A }`. Assert interrupt succeeds.
  Proves `Thread` is a process-wide resource keyed by `ThreadId`, not a connection-local artifact.
- **`thread_resume_concurrent.rs`** — Two clients call `thread/resume(thread_id_X)` in parallel.
  Assert second receives `Conflict` error (first already holds it). Validates split-brain prevention.
- **`compact_preserves_identity.rs`** — Submit `Op::Compact` on thread T. Assert `T.id` and
  `T.session_id` unchanged; transcript file `<T.id>.jsonl` gains a `RolloutItem::Compacted` line;
  no new thread allocated. Confirms D8.

### 8.3 Load test (`tests/load_concurrent_threads.rs`)

- 16 concurrent stdio-to-uds clients, each creating a thread + running 4 turns of `echo $((1+1))`
  via Bash tool against a stub model provider.
- Assert: 0 overload errors, all 64 turns complete within 60s, p50 turn latency < 1s, p95 < 5s.
  Memory growth < 100 MB RSS delta from baseline (validates no thread-state leak on archive).

### 8.4 Manual smoke (`/run` skill, end-to-end)

- `coco serve --listen ws://127.0.0.1:7777` + hand-rolled `wscat` session: `thread/create` →
  `turn/start` → `turn/interrupt`. Inspect wire format.
- `coco serve --listen unix:///tmp/coco.sock` + new `coco connect /tmp/coco.sock` (from
  `coco-app-server-client`).
- `coco` (TUI) starts, confirms in-process handle path works end-to-end.

## 9. Out of Scope (Deferred)

- **Multi-account auth.** `AuthManager` stays single-account; type signature future-proofed
  (Arc-wrapped, ready to become `AuthRegistry`) but no per-thread account selection today.
- **`DashMap` for `ThreadManager`.** `RwLock<HashMap>` until measured contention with >30 active
  threads.
- **Sharded inbound processor.** Single task until measured contention.
- **Additional serialization scopes** (`Process`, `FsWatch`, `ThreadPath`, `CommandExecProcess`,
  `FuzzyFileSearchSession`). Add when concrete callers materialize.
- **Legacy SDK compat shim.** Per user direction. SDK clients must update in lockstep.
- **Per-thread cwd override via `tokio::sync::watch`.** Today: turn-boundary snapshot only (cwd is
  in the D6 immutable side). Promote to live-mutable if swarm/sub-agent workflows demand mid-turn
  cwd flips.
- **Hub multi-process aggregation.** Hub server today reads one process's connector; multi-process
  aggregation is a separate Hub feature.
- **Intra-thread session sequencing.** Explicitly rejected per D8 / §1.4. `/clear` is fork.

## 10. References

- codex-rs identity types: `protocol/src/thread_id.rs:11-29` (`ThreadId(Uuid)`),
  `protocol/src/session_id.rs:15-65` (`SessionId(Uuid)` + `From<ThreadId>` / `From<SessionId>`),
  `app-server-protocol/src/protocol/v2/thread_data.rs:105` (wire `Thread` with `id` + `session_id`
  + `forked_from_id`)
- codex-rs SessionId allocation rule: `core/src/session/session.rs:951-955` — sub-agent inherits
  `agent_control.session_id()`; root uses `SessionId::from(thread_id)`. This is the smoking gun
  for the physical-vs-logical split.
- codex-rs `InitialHistory::Cleared` treated as `New`: `core/src/session/session.rs:515-520`
- codex-rs wire envelope: `app-server-protocol/src/jsonrpc_lite.rs:1-3` (JSON-RPC-lite, no `"jsonrpc":"2.0"`)
- codex-rs transport framing: `app-server-transport/src/transport/stdio.rs:45-46` (NDJSON),
  `app-server-transport/src/transport/unix_socket.rs:7,79` (WS-over-Unix),
  `app-server-transport/src/transport/websocket.rs:259,350` (WS-over-TCP)
- codex-rs `/clear` flow: `tui/src/chatwidget/slash_dispatch.rs:167-169`,
  `tui/src/app/event_dispatch.rs:30-41`, `tui/src/app/session_lifecycle.rs:463-528`,
  `app-server/src/request_processors/thread_processor.rs:1100-1105`,
  `core/src/session/session.rs:515-520`
- codex-rs dispatch: `app-server/src/lib.rs:733-1060`, `app-server/src/message_processor.rs`,
  `app-server/src/request_serialization.rs`
- codex-rs thread store / rollout: `thread-store/src/local/`, `rollout/`
- coco-rs current single-session loop (to delete): `app/cli/src/sdk_server/handlers/mod.rs:239`
- coco-rs `QueryEngine` to refactor (D3): `app/query/src/engine.rs:90`
- coco-rs `session_runtime` slot to delete: `app/cli/src/session_runtime.rs:474`
- Companion docs: `event-hub/spec.md`, `agentteam-architecture.md`, `multi-provider-plan.md`
