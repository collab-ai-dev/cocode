# Multi-Session AppServer Refactor Design

Status: v6.3. v6.1 added the Process/Project/Session three-scope split
(§6.2 + §6.5) after a five-product survey of cwd/config scoping; v6.2
folded in the first adversarial-review fixes (event durability taxonomy
+ seq restart continuity, driver turn-spawn rule, Closing-reopen
semantics, lock taxonomy, mailbox/state-machine serialization split,
SDK handle-recovery signatures, orphan policy — D-38..D-44) and refined
MCP for multi-project (§16, D-45). v6.3 folds in the second adversarial
pass: spawn-owned lifecycle tasks (load/close survive caller
cancellation), seq crash-recovery skip-ahead, project-only
`ProjectServices` with a split config/services lifecycle, the
registry-initiated/driver-executed close contract, std-lock + snafu
convention alignment, subscribe replay atomicity, and the process
shutdown sequence — D-46..D-54.
Supersedes the v5 revision (see git history of this file); v5's locked
decisions are carried forward in §22 unless explicitly reversed there.
This doc owns the cross-cutting AppServer architecture and migration plan.
Stable type definitions migrate to the owning `crate-coco-*.md` /
crate `CLAUDE.md` files as the crates land; until then the definitions here
are authoritative drafts.

Backward compatibility is a non-goal throughout: the migration is a
rip-and-replace cut-over (§18), not a dual-stack transition.

## 1. Requirements

One AppServer process hosts many root conversation sessions concurrently
while preserving coco-rs identity, storage, and runtime semantics.

- Use `SessionId` as the only root conversation identity.
- Do not introduce `ThreadId`. Do not adopt a thread-store model.
- Keep JSONL session storage canonical (no event-sourced DB).
- Make all session-scoped requests explicit about `session_id`; no
  per-connection "active session" default.
- Host sessions across different project roots in one process; resolve
  project-scoped configuration (project/local settings, permission rules,
  hooks, skills, project MCP) against each session's cwd (§6.2, §6.5).
- Allow more than one surface to display the same session.
- Allow one connection to host more than one surface (reverses v5 decision
  #6 "1 client = 1 session"; see §22 D-24).
- Allow passive subscribers to observe a session concurrently.
- Enforce exactly one `Interactive` surface per session in v1: the second
  attach is rejected with a typed error (§11.3). Takeover is v2.
- Keep AppServer core protocol-neutral behind `ProtocolAdapter`; keep byte
  and frame I/O behind `Transport`.
- Keep browser, desktop, and IM platform logic (credentials, tokens,
  callback capabilities) outside AppServer core.
- v1 is local-first: no public TCP or WebSocket listener enabled by default.
- v1 TUI stays single-session even though the server hosts many sessions.
- Engine and core crates never depend on server/protocol crates (§5.3).

## 2. Background — Current State

coco-rs already treats a session as the durable conversation unit: one
`SessionId`, one root JSONL transcript, message history, permission state,
memory, usage, pending work, event stream. That identity must stay stable
across resume, archive, reconnect, and client changes.

The current runtime is architecturally single-root-session-per-process, and
the seams this design must cut are concrete:

- `SessionRuntime` (`app/cli/src/session_runtime.rs`, ~4300 LoC) fuses
  process-lifetime resources (model runtimes, tool registry, hook registry,
  `Arc<RuntimeConfig>`, session manager) with per-session mutable state in
  one struct. The split exists only as a comment
  (`session_runtime.rs:549` "mutable per-session state").
- Session identity is still mutable:
  `QueryEngineConfig.session_id` is still repointed in place through the
  legacy `retarget_session_id_boundaries` helper on `/clear`
  (`clear_conversation`, new UUID path) and on SDK `session/start`
  / `session/resume` / TUI `/resume` (`retarget_for_loaded_session`). The
  file-history sink now derives its session id from the synchronized engine
  config mirror instead of maintaining its own retargeted identity.
- The SDK server enforces a singleton: `SdkServerState.session:
  RwLock<Option<SessionHandle>>` (`app/cli/src/sdk_server/handlers/mod.rs:236`,
  "Only one concurrent session per server"). The wire vocabulary already
  carries `session_id` on most requests; the state machine ignores it.
- `CoreEvent` (`common/types/src/event.rs`) has no `session_id` /
  `turn_id` on the envelope. History `ServerNotification` variants now share
  a flattened `ServerNotificationIdentity` for `session_id` / `agent_id`
  access while preserving the existing wire fields until AppServer stamping
  subsumes them. `AgentStreamEvent` deltas and turn lifecycle params now carry
  typed `TurnId`.
- The `SessionId` newtype (`common/types/src/id.rs:9`) has been adopted by
  `QueryEngineConfig.session_id`, staged compact identity, and transcript
  session identity. Runtime identity reads now go through
  `QueryEngineConfig.session_id`; that field remains mutable until the runtime
  split removes in-place id rotation.
- `QueryEngine` is already sink-agnostic: the event channel is passed per
  run (`run_with_events`, `app/query/src/engine_session.rs:57`), which fits
  server-owned routing with no changes to the engine's sink interface.
- Project and local settings layers are cwd-derived
  (`project_settings_path(cwd)` / `local_settings_path(cwd)`,
  `common/config/src/global_config.rs:206-214`) but are folded exactly
  once at process boot into one `RuntimeConfig`. Hosting sessions with
  different cwds turns the project layer (permission rules, hooks,
  sandbox, MCP servers) into per-session input — resolved by the
  per-session fold in §6.5.
- Query-layer runtime cwd reads have been moved behind temporary
  session-cwd helpers (`QueryEngineConfig::workspace_cwd`,
  `ToolUseContext::cwd_anchor` / `effective_shell_cwd`), but live
  `std::env::current_dir()` fallbacks still exist across wider tool and
  path-resolution boundaries (for example
  `utils/absolute-path/src/absolutize.rs:16`). With two sessions in
  different directories, any remaining process-cwd fallback can resolve
  against whichever cwd the process happens to have — removed by the
  §6.5 discipline.
- The Hub crates encode a **conflicting** identity model: per-instance
  global `seq` and a single per-connection resume cursor
  (`hub/protocol/src/lib.rs:39,51,63`), and `event-hub/spec.md` §4 assumes
  one live session per process with id rotation. §13 reconciles this.
- `coco-state::AppState` is declared but not wired (`app/state/src/lib.rs:130`).
  The live per-session state container is `ToolAppState`
  (`session_runtime.rs:580`). This design uses `ToolAppState`; it does not
  route through `coco-state::AppState`.
- `coco-coordinator` is a multi-*agent* registry under one root session
  (teams/swarm, keyed by `AgentId`). The `LiveSessionRegistry` sits one
  level above it; the two registries layer and never merge (§6.6).

## 3. Reference Products — what we take and what we avoid

**Codex app-server** (`/lyz/codespace/3rd/codex`). Take: macro-generated
typed protocol with JSON-Schema/TS export and per-method/per-field
experimental gates (`app-server-protocol/src/protocol/common.rs:192`);
server-initiated request correlation with per-thread pending-request
cancellation and replay-on-reattach (`app-server/src/outgoing_message.rs`);
one listener task per thread fanning out to the current subscriber set;
bounded reported shutdown; slow-consumer disconnect
(`app-server/src/transport.rs:154`). Take: the config scoping model — a
process-global `ConfigManager` builds a fresh layered `Config` per
thread at `thread/start` (`app-server/src/config_manager.rs:187`), with
a Project layer (`.codex/config.toml`, precedence 25, discovered from
the thread's cwd → git root) between the User layer (20) and per-thread
`SessionFlags` overrides (30) (`config/src/config_layer_source.rs:43`),
plus a `[projects."/path"]` trust table keyed by git root
(`config/src/config_toml.rs:428`). Take: explicit cwd threading —
`ThreadStartParams.cwd` → per-thread config → `TurnEnvironment.cwd` →
`cmd.current_dir(cwd)` (`core/src/spawn.rs:71`), with `TurnContext.cwd`
marked `#[deprecated]`; we enforce the same rule by lint where codex
relies on convention (§6.5). Avoid: `core` depends on
`codex-app-server-protocol` (`core/Cargo.toml:29`) — the engine imports
wire-view types, inverting the layering this doc mandates (§5.3). Avoid:
its resume dedup is optimistic (lock dropped before spawn,
`thread_manager.rs:1527-1548`) — a duplicate spawn is possible and
discarded late; our `Loading` slot is the true single-flight fix. Avoid:
capabilities scoped per connection on shared threads (their own TODO at
`request_processors/initialize_processor.rs:63`); we scope capabilities
per surface (§11.4).
We do not adopt `ThreadId`.

**jcode** (`/lyz/codespace/3rd/jcode`). Take: server-owned sessions with
client-owned surface state; single interactive owner per session with a
typed rejection carrying retry hints (`client_session.rs:1199`), plus
multi-attach event fan-out for passive viewers (`state.rs:393`); UDS /
named-pipe local transport. Take: per-session `working_dir` threaded as
`ToolContext.working_dir` into every child spawn
(`crates/jcode-tool-core/src/lib.rs:30`,
`tool/bash.rs:656` `command.current_dir(dir)`) — the server process is
project-agnostic (global socket registry under `~/.jcode`). Note for v2:
its owner-takeover predicate and eviction flow (`client_session.rs:981`)
is the model for interactive takeover. Avoid: `Arc<Mutex<Agent>>` as the
session state container — one coarse mutex serializes reads, writes, and
turn state; we use an actor split instead (§6.3). Avoid: its
config-is-process-global model (only `model`/`working_dir` vary per
session, no project config layer) — too coarse for coco's project-scoped
permissions and hooks.

**opencode** (`/lyz/codespace/3rd/opencode`). Take: per-session replayable
event streams keyed by a per-aggregate monotonic seq, separate from the
live push channel; snapshot + `after=seq` replay as the reconnect story.
Take: the per-directory service container — `LocationServiceMap` builds
~30 directory-scoped services (config, permissions, tools, watcher)
fresh per `Location.Ref` with 60-minute idle eviction
(`packages/core/src/location-services.ts:84`); sessions persist their
own directory and every tool resolves against it with containment checks
(no `process.chdir` anywhere). This is the shape of our
`ProjectServices` cache (§6.2). Avoid: the unfiltered global SSE stream
as the local default; avoid the event-sourced SQLite store (JSONL stays
canonical).

**OpenClaw** (`/lyz/codespace/3rd/openclaw`). Take: the **two-level
identity** insight — a stable channel routing key maps to the *current*
internal session id, and `/new` mints a fresh internal id under the same
key (`session-reset-service.ts` `buildNextEntry`). That is structurally
our `session/replace`. The IM gateway therefore owns a durable
`channel key → SessionId` map and re-points it on replace (§12.5). Take:
channel capability declaration per connector. Keep out of core: platform
tokens, allowlists, per-platform rate limits.

**Hermes Agent** (`/lyz/codespace/3rd/hermes-agent`). Take: the trust
boundary — platform secrets and callback capabilities live only in the
connector/gateway; the agent core wields them via token-less references
(`session_key` + capability kind). Take: session-keyed interrupts routed
over the gateway back-channel. Avoid: its split-brain cwd — the shell
tool roots on the process-global `TERMINAL_CWD` while file discovery
honors a per-session ContextVar pin (`agent/runtime_cwd.py`), two cwd
semantics in one process; §6.5 mandates one per-session cwd source of
truth for all consumers.

## 4. Identity Model

### 4.1 SessionId is the only root identity

`SessionId` (`coco_types`, `common/types/src/id.rs:9`) is used by protocol
requests, event routing, transcript lookup, live-session routing, and
client resume. Server-generated (UUID v4 string); clients cannot propose.
There is no second root identity and no per-connection default.

**Typed-adoption mandate.** The refactor eliminates the competing
representations: legacy string config fields, UUID staging fields, and
the runtime's mutable id lock. After cut-over,
`SessionId` is a plain immutable field everywhere; anything that "changes
the session id" creates a new runtime (§7). The same mandate applies to
`TurnId`: `AgentStreamEvent` and `ThreadItem` switch from `String` to
`TurnId` (`event.rs:118,120,172`).

`SessionId` and `AgentId` remain validated path-safe newtypes (private
inner, fallible serde; reject separators, `.`/`..`, empty).

### 4.2 New identifiers

- `TurnId` — exists (`id.rs:173`). Minted by the session runtime at turn
  start; every turn-scoped request and notification carries
  `(session_id, turn_id)`.
- `SurfaceId` — new newtype in `coco_types`. Server-generated per
  `SurfaceAttachment`. Public on the wire (clients reference their own
  surfaces), never persisted.
- `ConnectionKey` — internal server state only: routing, subscription
  cleanup, pending-request cancellation, transport lifecycle. Never on the
  wire, never persisted.

### 4.3 Subagents

Unchanged: subagents are `AgentId` under their parent root session, stored
at `<session_id>/subagents/agent-<agent_id>.jsonl`. They are not registry
entries; they inherit the parent session's cancellation tree and resources
and never widen features.

## 5. Crate Boundaries

### 5.1 New crates

| Crate | Error tier | Contents |
|---|---|---|
| `app/runtime` → `coco-app-runtime` | Tier 3 (snafu + `coco-error`) | `ProcessRuntime`, `SessionRuntime` (driver task), `SessionHandle`, `SessionRuntimeFactory`; per-turn `QueryEngine` construction; transcript/history/command-queue/turn wiring extracted from `app/cli/src/session_runtime.rs` |
| `app/server` → `coco-app-server` | Tier 3 | `AppServer`, `LiveSessionRegistry`, connection + surface registries, subscriptions, pending server requests, envelope stamping + fan-out + retention ring (§10), serialization model (§9), `LocalClientAdapter` + `JsonRpcAdapter` (§12), lifecycle + graceful drain |
| `app/server-transport` → `coco-app-server-transport` | Tier 2 (thiserror) | stdio NDJSON, UDS, WebSocket framing + the JSON-RPC frame types (§5.2); connection acceptance, backpressure, close detection. Pure I/O — no coco domain state; yields accepted connections to `coco-app-server`, which assigns `ConnectionKey` |
| `app/server-client` → `coco-app-server-client` | Tier 2 (thiserror; no `coco-error` in public API) | in-process `LocalTransport` (typed, no serde) for TUI; UDS/WS `RemoteTransport` (JSON-RPC) for SDK; `ServerClient` / `SessionClient` (§14) |

Phase-5 adapters (`WebAdapter`, `DesktopAdapter`, `ImGatewayAdapter`) land
as separate crates (`coco-app-web`, `coco-app-desktop`,
`coco-app-im-gateway`) depending on `coco-app-server`'s canonical types;
they are out of v1 scope but their boundaries are fixed now (§12).

### 5.2 Deferred crate

No `app/server-protocol` crate in v1. Canonical `ClientRequest`,
`ServerNotification`, and `SessionEnvelope` live in `coco-types`. The
JSON-RPC frame types (request/response/error envelopes, ids) are wire
format, not domain — they live in `coco-app-server-transport`, shared
by the server-side `JsonRpcAdapter` and the client-side
`RemoteTransport`, keeping wire artifacts out of the foundation crate.
Reconsider a dedicated protocol crate once the surface stabilizes and
schema generation (§8.2) makes codegen worthwhile.

### 5.3 Layering constraint (hard rule)

Engine and core crates (`app/query`, `core/*`, `services/*`) MUST NOT
depend on `coco-app-server*`. Shared view types (envelopes, notification
payloads, ids) live in `coco-types`, below both. This is the codex
counter-lesson (§3): their `core` → `app-server-protocol` dependency lets
the wire view leak into the engine. `scripts/` gains a seam-guard check
mirroring `check-tui-ui-seam.sh`.

## 6. Runtime Split

Ownership is three nested scopes, not two. Every reference product that
hosts concurrent work in one process supports multiple working
directories (codex per-thread cwd, opencode per-directory service map,
jcode per-session `working_dir`, openclaw per-session `spawnedCwd`);
none binds one process to one project root. coco-rs follows: the middle
scope exists because project-derived state (settings layers, permission
rules, hooks, project MCP) is neither process-global nor per-session.

```text
ProcessRuntime            (one per process)
  └─ ProjectServices      (cached per project root, §6.2)
       └─ SessionRuntime  (one per SessionId, §6.3)
```

### 6.1 ProcessRuntime

Constructed once per AppServer process. Owns what is genuinely
user/process-level and publishes snapshot-style handles:

- settings layers that do not depend on a cwd — policy, user, flag, env
  (`EnvSnapshot`) — plus `CatalogPaths`: the process-side inputs to the
  per-session config fold (§6.5). There is no process-wide
  `Arc<RuntimeConfig>` anymore; `RuntimeConfig` is a per-session
  snapshot.
- model/client factories (`ModelRuntimeRegistry`), model registry,
  provider catalog, auth/OAuth state, keyring. Cached provider clients
  are keyed by `ProviderClientFingerprint`: per-session folds (§6.5)
  can resolve different provider options per project, and two sessions
  must never share a client built from a different fold
- built-in tool registry; bundled + user-level sources for skills,
  commands, hooks, output styles (project-level contributions overlay in
  `ProjectServices`)
- user-level MCP catalog (§16); project `.mcp.json` contributions live
  in `ProjectServices`
- `ProjectRegistry` — the `ProjectServices` cache (§6.2)
- protocol adapter registry, transports
- `SessionManager` (JSONL catalog, `app/session`)

**Snapshot-at-session-start:** session creation folds the current
process layers with the session's project layers (§6.5) and clones the
relevant `Arc<T>` handles into the new runtime. Running sessions never
re-read process or project slots; `settings/update` / `plugin/reload`
mutate process slots via `ArcSwap::store` / `watch::Sender::send` and
affect only sessions created afterwards.

### 6.2 ProjectServices — shared services per project root

Everything derived from a project directory (rather than from the user
or the session) lives in one container, cached per project root. This is
the opencode `LocationServiceMap` shape (fresh per-directory service
group, idle eviction) carrying codex's config semantics (project layer +
trust resolved from cwd → git root).

```rust
struct ProjectServices {
    project_root: AbsolutePathBuf,        // git worktree root, else session cwd
    config: ProjectConfigSnapshot,        // cheap; freshness-checked per session start
    services: ProjectHeavyServices,       // expensive; live for the entry's lifetime
}

/// Project-level contributions ONLY — never pre-merged with process
/// sources. The per-session fold (§6.5) merges process + project at
/// session/start, so a process-layer reload (§6.1) reaches every new
/// session even in an already-cached project.
struct ProjectConfigSnapshot {
    settings: ProjectSettingsLayers,      // project + local settings snapshots
    permission_rules: Arc<ProjectPermissionRules>,
    hooks: Arc<ProjectHookSet>,           // project hooks only
    skills: Arc<ProjectSkillSet>,         // project skills/commands only
    mcp: ProjectMcpCatalog,               // .mcp.json contributions
    fingerprint: SettingsFingerprint,     // mtime/len of the source files
}

struct ProjectHeavyServices {
    context: ContextDiscoveryCache,       // CLAUDE.md discovery
    ignore: Arc<FileIgnoreService>,
    lsp: Option<Arc<LspManager>>,         // rooted at project_root
    retrieval: Option<Arc<RetrievalFacade>>,
    // project-defined `Shared` MCP instances also live here (§16.2)
}

struct ProjectRegistry {
    projects: std::sync::RwLock<HashMap<AbsolutePathBuf, ProjectEntry>>,  // §7.5 lock rules apply
}
```

- `project_root = resolve_project_root(cwd)`: git worktree root, else
  the cwd itself. This is the same derivation session storage already
  uses for its `projects/<slug>/` layout — the two must not diverge.
- Lifecycle: `get_or_load(project_root)` at session start, single-flight
  like the session registry (§7.2). Entries are ref-counted by attached
  sessions and evicted after `server.project_services_idle_ttl_secs`
  with zero sessions (§17).
- Two sessions in the same project share one instance; two sessions in
  different projects get fully independent instances — permission rules,
  hooks, and skills differ per project by design.
- Project trust/onboarding stays keyed by project root in user-level
  state (the codex `[projects."/path"]` pattern; coco already has
  `maybe_mark_project_onboarding_complete`,
  `common/config/src/global_config.rs:274`).
- Freshness is checked, not assumed: `get_or_load` stats the snapshot's
  source files against `fingerprint` and re-reads a stale
  `ProjectConfigSnapshot` in place before the fold uses it. This is
  what makes §6.5's "a resumed session sees current settings files"
  true even while other sessions keep the entry alive.
  `ProjectHeavyServices` (LSP, retrieval, project-`Shared` MCP) are
  deliberately NOT recycled by a config re-read — they live until the
  entry evicts (§17). Sessions already running keep their own fold
  snapshot; re-reads affect only subsequent session starts.

### 6.3 SessionRuntime — the actor that owns one session

One `SessionRuntime` per live root session, driven by a dedicated tokio
task (the *driver*). The driver exclusively owns the mutable session
state:

- `Arc<RuntimeConfig>` — the per-session fold snapshot (§6.5), including
  the session's resolved `Arc<Features>`
- `Arc<ProjectServices>` — shared, read-only from the session's
  viewpoint (§6.2)
- session `cwd` (`watch::Sender`; receiver exposed on the handle)
- `MessageHistory`, transcript writer (`TranscriptStore`)
- `ToolAppState` (`Arc<RwLock<ToolAppState>>` — shared with the per-turn
  engine and tools *within* the session's ownership domain only; never
  across sessions, never locked by the server)
- command queue (pending turns, steering inbox)
- active turn state (`TurnId`, per-turn `QueryEngine` built from process
  snapshots — the existing per-turn rebuild pattern is kept)
- usage tracker, session memory, session permission rules
- session-scoped MCP runtime handles (per `McpScope`, §16)
- `worktree_state`
- `task_set: JoinSet<()>` for session-spawned background work (background
  agents, teammate runner loops register here at spawn)
- event sink: `mpsc::Sender<CoreEvent>` handed to each engine run
  (`run_with_events`) — the receiving end is owned by AppServer routing,
  which stamps the envelope (§10.1)

The driver consumes `SessionCommand` messages; requests never reach into
session state from server threads. This is the codex `CodexThread` shape
and the explicit rejection of jcode's `Arc<Mutex<Agent>>` coarse lock and
of the current `SessionRuntime`'s several-dozen `Arc<RwLock<_>>` fields.

**The driver never runs a turn inline — correctness rule, not style.**
Every mailbox command is fast (validate, enqueue, signal, snapshot);
none awaits turn completion. `StartTurn` validates, mints the `TurnId`,
spawns the turn as its own task (registered in `task_set`, token
descended from `session_token`), flips `status` to
`TurnActive { turn_id, .. }`, replies, and returns to the loop. Commands
keep flowing while the turn runs: `Interrupt` fires the turn's token,
`ReadState` answers from driver state, `Steer` feeds the running turn's
inbox. A driver that awaited the turn inline would starve the mailbox
and make interrupt undeliverable. Turn completion re-enters the driver
as an internal message that flips status and starts the next queued
turn.

```rust
enum SessionCommand {
    StartTurn { params: TurnStartParams, reply: oneshot::Sender<Result<TurnId, TurnError>> },
    Interrupt { turn_id: Option<TurnId>, reply: oneshot::Sender<Result<(), TurnError>> },
    Steer { input: SteerInput, reply: oneshot::Sender<Result<(), TurnError>> },
    ReadState { query: SessionStateQuery, reply: oneshot::Sender<SessionStateView> },
    UpdateRuntime { patch: SessionRuntimePatch, reply: oneshot::Sender<Result<(), SessionError>> },
    Close { reply: oneshot::Sender<()> },
}
```

The mailbox is bounded (fixed capacity 64 — every command is fast, so
it drains quickly) and senders `send().await`; there is no
try_send-and-drop path, so `Interrupt` can be delayed briefly but never
lost. `Close` is not a client-reachable command: only the registry's
close path issues it (§7.4) — it is the mechanism by which lifecycle
operations stop the driver, while lifecycle *arbitration* stays in the
slot state machine (§9).

### 6.4 SessionHandle — the cheap facade

`SessionHandle` is what the registry stores and clones. It is immutable
with respect to identity: clear, resume, replace, archive never mutate a
handle into representing a different session — they create, load, swap, or
close registry entries (§7).

```rust
#[derive(Clone)]
pub struct SessionHandle {
    session_id: SessionId,                  // plain field — no lock
    created_at: DateTime<Utc>,
    commands: mpsc::Sender<SessionCommand>,
    status: watch::Receiver<SessionStatus>, // Idle | TurnActive { turn_id, queued } | Draining
    cwd: watch::Receiver<AbsolutePathBuf>,
}
```

The session root `CancellationToken` is deliberately NOT on the handle:
it is private to the runtime and the registry's close path. A handle
holder cancels work by sending `Interrupt` or by going through registry
`close` — never by bypassing the lifecycle state machine with a raw
token. `SessionStatus::TurnActive` carries the pending-queue depth so
status surfaces (opencode-style busy/queued indicators) need no mailbox
round-trip.

No surface state, no viewport state, no notification preferences, no
cursors live here — those belong to AppServer surface/connection
registries (§11).

**Concurrency contract:**

- Registry map lock (`std::sync::RwLock`, §7.5) is held only to
  snapshot or mutate the map. No `.await` under the lock — structural,
  not just convention: the guard is sync. The two documented patterns
  are Loading insertion (placeholder in, lock released, await outside —
  §7.2) and the replace commit (single write section, zero awaits —
  §7.5).
- Cross-session state sharing is forbidden. Anything shared is either a
  process snapshot (`Arc<T>`, read-only) or routed as a message.
- Background tasks spawned by a session `.instrument(session_span.clone())`
  so `session_id` survives spawn boundaries (OTel fields per
  `common/otel/CLAUDE.md`).

### 6.5 Per-session configuration fold and cwd discipline

**Config fold.** `coco_config::build_runtime_config_with(settings, env,
overrides, catalogs, sources)` (`common/config/src/runtime.rs:322`)
stays the single merge site. What changes is the call site and its
inputs: it runs once per `session/start` (and per resume), not once per
process boot:

```text
session/start { cwd, overrides }
  → project_root  = resolve_project_root(cwd)
  → project       = ProjectRegistry.get_or_load(project_root)
  → settings      = process layers (policy, user, flag, env)
                  + project.settings (project, local)
  → RuntimeConfig = build_runtime_config_with(settings, env_snapshot,
                      session_overrides, catalogs, sources)   // snapshot
```

- `SessionStartParams` overrides (model role, permission mode, thinking)
  sit above all file layers — the codex `SessionFlags` position.
- `Features` resolve inside the fold, so they are per-session values
  that may legitimately differ across projects. There is still no
  `SessionStartParams.features` override, and subagents inherit the
  parent `Arc<Features>` and never widen.
- Permission evaluation keeps its own more-specific-wins order
  (session > … > projectSettings > userSettings > policySettings); the
  fold only determines *which* project layer feeds it.
- Resume re-runs the fold against the session's recorded cwd — a resumed
  session sees current settings files, not the files as of its first
  start. The snapshot is taken whenever the runtime is (re)constructed.

**cwd discipline.** The OS process has one cwd; sessions have many. One
per-session cwd is the single source of truth for every consumer:

- Session cwd comes from `SessionStartParams.cwd` (required, absolute,
  must exist), lives in session state, and is exposed on the handle as a
  `watch` snapshot. Tools receive it via `ToolUseContext`; every child
  process is spawned with an explicit `current_dir` (jcode
  `ToolContext.working_dir` / codex `TurnEnvironment.cwd` pattern).
- Relative-path resolution happens against the session cwd, never the
  process cwd. The live `std::env::current_dir()` fallbacks on
  session-scoped paths (§2) are deleted; `absolutize` gains an
  explicit-base variant for session paths.
- Steady-state enforcement is a lint, not a convention: `clippy.toml`
  adds `std::env::current_dir` to `disallowed-methods`, with a narrowly
  scoped `#[allow(clippy::disallowed_methods)]` at the CLI entrypoint
  (initial cwd capture) and nowhere else. Until the remaining standalone
  tools and path utilities can be allow-listed or split cleanly, the
  checked-in `check-session-cwd-discipline.sh` guard enforces the same
  rule on session-owned production code.

### 6.6 Layering over coco-coordinator

`LiveSessionRegistry` manages root sessions; `coco-coordinator`'s
`TeamManager` / agent handles manage agents *within* one root session. The
coordinator instance is session-owned state (driver-owned). Merged-timeline
demux stays `(session_id, agent_id)`, now uniformly available on the
envelope (§10.1) rather than only on history variants.

## 7. LiveSessionRegistry

### 7.1 Shape

```rust
struct LiveSessionRegistry {
    sessions: std::sync::RwLock<HashMap<SessionId, SessionSlot>>,
    max_sessions: usize,
}

enum SessionSlot {
    /// Single-flight load/resume; concurrent callers await the same
    /// completion signal.
    Loading(SharedLoadFuture),
    /// Serving requests.
    Live(SessionHandle),
    /// Close cascade running. In-flight work may drain; new `resume`
    /// callers await the close future, then reopen from disk (§7.3).
    Closing(SessionHandle, SharedCloseFuture),
}

/// Completion signals, NOT the work itself. The load and the close
/// cascade each run in their own spawned owner task, which also
/// performs the slot transition (§7.2, §7.4); these futures merely
/// broadcast the outcome (`Shared` over the owner task's completion
/// channel). A caller-driven future would stall unpolled if every
/// awaiting request were cancelled — wedging the slot forever with the
/// load half-done. Owner tasks make slot progress independent of
/// callers: zero remaining awaiters is fine.
type SharedLoadFuture =
    futures::future::Shared<BoxFuture<'static, Result<SessionHandle, ResumeError>>>;
type SharedCloseFuture =
    futures::future::Shared<BoxFuture<'static, ()>>;
```

`ResumeError` MUST be `Clone` — `futures::future::Shared` requires the
result to be cloneable so every concurrent caller receives the same error.
Therefore no `std::io::Error` source field (not `Clone`); convert at
the load site into `(kind, message)`:

```rust
#[derive(Debug, Clone, Snafu)]
enum ResumeError {
    #[snafu(display("session not found: {session_id}"))]
    NotFound { session_id: SessionId },
    #[snafu(display("max_sessions limit reached"))]
    ResourceExhausted,
    #[snafu(display("session id format invalid: {raw}"))]
    Invalid { raw: String },
    #[snafu(display("transcript load failed ({kind:?}): {message}"))]
    LoadFailed { kind: TranscriptLoadKind, message: String },
    #[snafu(display("recorded cwd no longer exists: {}", recorded_cwd.display()))]
    CwdNotFound { recorded_cwd: PathBuf },
}

#[derive(Debug, Clone, Copy)]
enum TranscriptLoadKind { Io, ParseError, MissingHeader, Truncated }
```

All registry error enums use snafu derives and implement
`coco_error::StackError + ErrorExt` with a `StatusCode` — Tier 3 has
exactly one allowed error library, and a `thiserror` derive here would
trip `check-error-policy`. `thiserror` remains correct in the Tier-2
crates (`coco-app-server-transport`, `coco-app-server-client`).

### 7.2 Single-flight load

When `session/resume(id)` finds neither `Live` nor `Loading`:

1. Acquire write lock; re-check; spawn the load task; insert
   `Loading(completion)`; release lock.
2. The spawned task owns the load end-to-end and performs the slot
   transition itself — success: write lock, swap
   `Loading → Live(handle)`; failure: write lock, remove the entry.
   Progress never depends on any caller staying alive.
3. Callers — the originator included — only await the completion
   signal, outside any lock. Concurrent callers that observe `Loading`
   await the same signal; a caller cancelled mid-await affects nothing;
   zero remaining awaiters cannot wedge the slot.
4. After a failure the entry is already gone; the next caller retries
   from scratch.

This is a true single-flight — codex's optimistic variant (dedup check,
lock dropped, spawn anyway, discard duplicate at insert) wastes a full
session spawn under race and is explicitly not copied.

`Loading`, `Live`, and `Closing` all count toward `max_sessions`. The one
exception is `session/replace`, which transiently occupies +1 (§7.5).

### 7.3 Lifecycle operations

```rust
async fn create(params: SessionStartParams) -> Result<SessionHandle, CreateError>;
async fn resume(id: SessionId, params: SessionResumeParams) -> Result<SessionHandle, ResumeError>;
async fn replace(old: SessionId, params: SessionStartParams) -> Result<SessionHandle, ReplaceError>;
async fn close(id: SessionId) -> Result<(), CloseError>;   // wire op: session/archive
fn get(id: &SessionId) -> Option<SessionHandle>;
fn list_live() -> Vec<SessionId>;
```

State machine:

```
                       single-flight resume
                 ┌──────────────────────────┐
                 ▼                          │
session/start → Live ──── archive ────► Closing ──► (removed)
                 ▲                          │
                 │                          │ resume(id) during Closing:
session/resume(loadable on disk)            │ await close future, then
  creates Loading ──success──┘              ▼ reopen from disk (Loading)
  creates Loading ──failure──► (removed)
```

Invariant: one `SessionId` occupies at most one slot at any instant.

- **Create**: mint a new `SessionId` and insert it as `Loading`
  immediately — construction is not free (config fold §6.5, transcript
  file creation, PerSession MCP spawn) and must occupy a `max_sessions`
  slot while it runs. Promote to `Live` on success, remove on failure —
  exactly the resume single-flight shape.
- **Resume, live id**: return the existing handle (multi-surface rejoin).
- **Resume, on-disk id**: single-flight load (§7.2), reconstruct from
  JSONL, promote to `Live`.
- **Resume during `Closing`**: a closing runtime cannot serve — its token
  is cancelled and its driver is draining, so handing out the old handle
  would only produce command errors. The resolver awaits the
  `SharedCloseFuture` (outside any lock), then re-enters the normal
  on-disk path: insert `Loading`, reconstruct from JSONL. Reopen, not
  rejoin.
- **Archive** (wire name kept; slot state `Closing`): runtime close, not
  transcript deletion. The JSONL stays on disk and is re-openable via
  `session/resume`.
- **Close on `Loading`**: await the load future outside any lock. If the
  load failed the entry is already gone — return `Ok(())`. If it
  succeeded, transition `Live → Closing` and run the cascade. Finishing
  the in-flight load and then closing cleanly beats aborting IO that is
  already happening.

### 7.4 Close cascade (ordered)

The cascade is **registry-initiated, driver-executed,
supervisor-completed**. The registry close path cancels the token,
sends `SessionCommand::Close`, and spawns a supervisor task that awaits
the driver's `JoinHandle`. The drain steps run inside the driver
because they touch driver-owned state (queue, transcript writer,
`task_set`) that nothing else may reach; the supervisor performs the
final step and completes the `SharedCloseFuture`. Like the load task
(§7.2), close progress never depends on the caller of `close()`
staying alive.

1. Registry: cancel `session_token` (cascades to all turn and subagent
   tokens); send `SessionCommand::Close`; spawn the supervisor.
2. Driver: drop the pending-turn queue; emit `turn/interrupted` per
   queued turn.
3. Driver: wait for the active turn to reach its drain point, bounded
   by `server.turn_drain_timeout_secs` (§17). On timeout, proceed —
   step 6 aborts whatever is still running; a wedged tool must not pin
   the slot in `Closing` forever.
4. Driver: SIGTERM PerSession MCP children; 5 s grace; SIGKILL (§16).
5. Driver: flush the transcript writer (await pending writes; persists
   the `session_seq` watermark, §10.3).
6. Driver: `task_set.shutdown().await` — abort-join backstop
   (`JoinSet::shutdown` aborts remaining tasks, then awaits them;
   well-behaved tasks already exited on the step-1 token). No
   `Arc::strong_count` polling. The driver task returns.
7. Supervisor: remove the registry entry; emit `session/ended`; detach
   surfaces (§11.5); complete the close future.

### 7.5 Replace — two-phase commit

`/clear` and SDK replace are this primitive. Old stays fully operational
until new is committed; a build failure rolls back to nothing. The
entire operation — all three stages — runs in one spawned owner task
(the §7.2 principle); the caller only awaits its completion signal, so
a cancelled caller can wedge neither slot nor leave the commit half
done.

```
Stage 1 — build new (caller awaits a completion signal):
  a. Write lock: verify old is Live(handle_old) else ReplaceError::OldNotReady;
     mint new_id; insert new_id → Loading(completion). Release lock.
  b. The owner task constructs the new runtime (old keeps serving
     turns, MCP, reads).
  c. On failure the owner task removes the new_id entry; the caller
     gets ReplaceError::ConstructFailed — old untouched.

Stage 2 — atomic commit (single write-lock section, zero .await):
  d. Swap new_id: Loading → Live(new_handle).
  e. Re-mark old: Live → Closing(handle_old, close_future).
  f. Re-point the calling surface's attachment old → new (§11.5), so the
     caller never observes a routing gap.
  g. Release lock; emit session/started(new_id) to the caller;
     emit session/replaced { old, new } to all other surfaces on old.

Stage 3 — background close:
  h. Run the §7.4 cascade on handle_old; on completion remove the entry
     and emit session/ended(old_id).
```

Rollback matrix:

| Failure point | New | Old |
|---|---|---|
| Stage 1 construct | Loading entry removed | Live, fully intact |
| Stage 2 | unreachable (no `.await` in the commit section) | — |
| Stage 3 cascade | Live (committed) | Closing; cascade is re-runnable |

**max_sessions accounting:** replace transiently occupies 2 slots
(Stage 1: Live+Loading; Stage 2–3: Closing+Live) and therefore bypasses
`max_sessions` by +1 for its own duration — it is a swap, not a capacity
grant. A concurrent `session/start` still sees the full limit.

**Lock taxonomy.** The commit section touches two structures — the
session map and the surface routing state — guarded by exactly two locks
with a fixed order:

- `LiveSessionRegistry.sessions` (`std::sync::RwLock`) — slot states only.
- `RoutingState` (`std::sync::RwLock` over connections, surfaces,
  forward + reverse maps, pending server requests, and the per-session
  retention rings §10.2) — all cheap O(1) map mutations and `try_send`s.

Both are std locks, not tokio locks, on purpose: the no-`.await` rule
makes an async-aware lock pure overhead, a sync guard makes holding one
across an await a visible mistake (`!Send` guard in a spawned task),
and §7.3's synchronous `get` / `list_live` signatures follow directly.

Rule: registry lock before routing lock, never the reverse, and no
`.await` under either. Stage 2 takes both in that order for the
swap + re-point; every other code path takes at most one. Keeping all
routing metadata under ONE lock is deliberate — the maps are small and
mutations constant-time; splitting them invites ordering bugs for zero
measured win.

If Stage 2 ever needs to persist state (e.g. writing `Closing` to disk),
it can no longer be a single lock section and must be redesigned — flag in
review.

### 7.6 Sessions without surfaces

Sessions are decoupled from connections by design — a session with zero
attached surfaces keeps running (unattended background agents are a
supported pattern). That makes leak policy explicit rather than
accidental:

- A session ends only via `session/archive`, `session/replace`, or
  process shutdown. Losing a connection, a surface, or an SDK handle
  never ends a session.
- Orphans are visible, not silent: `session/list` reports
  attached-surface counts and last-activity timestamps, so clients and
  `coco ps`-style tooling can find and archive abandoned sessions before
  `max_sessions` fills.
- Optional guard: `server.idle_session_timeout_secs` (§17, default off)
  archives a session that has had zero surfaces AND no active or queued
  turn for the configured duration. Off by default because unattended
  work is legitimate.

### 7.7 Process shutdown

SIGINT/SIGTERM (and `server/shutdown` on the local adapter) runs one
fixed sequence:

1. Stop accepting connections; close listening transports. Existing
   connections stay up to observe the drain.
2. Run the §7.4 close cascade on every `Live` session concurrently
   (`Loading` slots: await the load, then close — §7.3), bounded by
   `server.shutdown_timeout_secs` (§17). On timeout, remaining drivers
   are aborted — each cascade flushes its transcript before its abort
   backstop, so the JSONL stays consistent.
3. Flush hub connector egress under the same deadline.
4. Close remaining connections; exit 0 on clean drain, non-zero when
   the timeout forced aborts.

A second signal during the drain aborts immediately (standard
double-Ctrl-C semantics).

## 8. Protocol Model

### 8.1 Canonical requests

AppServer core receives canonical typed requests (`ClientRequest` in
`coco-types`), dispatches to registry/runtime/surface services, and emits
canonical notifications. Adapters own every protocol-specific shape.

Request-handling rules:

- Resolve `session_id` before touching any session state.
- Unknown / `Closing` / unauthorized session access → typed errors
  (`ResumeError::NotFound`, `SessionError::Closing`, …), never silent
  fallback.
- Turn-scoped requests carry `(session_id, turn_id)` once a turn exists;
  a mismatched pair is rejected (`WrongSession` / `WrongTurn`).
- Interactive turn starts serialize per session (§9); independent sessions
  run turns concurrently.
- Every session-scoped notification is enveloped with `session_id`; every
  turn-scoped notification additionally with `turn_id` (§10.1). Stream
  aggregation keys on `(session_id, turn_id)`.

Browse/read APIs are three-tier and paginated: `session/list` (summaries),
`session/read` (state snapshot + message page), `session/turns/list`.
Passive surfaces use exactly these for history — there is no separate
"snapshot for subscribers" API.

### 8.2 Typing and schema direction

v1 ships plain serde tagged enums in `coco-types`
(`#[serde(tag = "method", rename_all = "camelCase")]`), mirroring the
existing SDK dispatcher. When `WebAdapter`/`DesktopAdapter` need schema
and TS bindings, adopt the codex pattern: a single macro invocation as the
source of truth generating the enum, the typed dispatch payloads, the
capability/experimental gate tables, and JSON-Schema/TS export
(`common.rs:192-453` as the reference). Experimental gating is per method
and per field, enforced at one dispatch site.

### 8.3 Server-initiated requests

Permission prompts, elicitations, and desktop capabilities are server →
client requests. Mechanics (codex-derived):

- Server-minted monotonic request ids; pending map
  `RequestId → { oneshot reply, session_id, surface_id }`.
- Cancellation is precise: on surface detach or connection close, cancel
  that surface's pending requests; on turn transition, abort the turn's
  pending approvals; on session close, cancel all for the session.
- A late-attaching interactive surface gets still-open requests replayed;
  resolution is announced via `serverRequest/resolved`, ordered through
  the session's event stream so it cannot race the request itself.
- Approval replies are validated against `(session_id, prompt_id)`;
  mismatch → `WrongSession`.
- A prompt travels two channels with distinct roles (§10.1): the
  actionable server→client *request* goes only to the interactive
  surface that declared the capability (replayed on late attach); the
  durable *lifecycle notifications* (opened / resolved) flow through
  the envelope stream for passive surfaces, ring replay, and the Hub.
  Only the request channel can answer.

## 9. Serialization Model

Serialization comes from three mechanisms — not three look-alike queues:

- **Session order = the driver mailbox (§6.3).** Turn-scoped and
  session-state operations (`turn/start`, steering, session-scoped
  MCP/runtime updates, rewind/file-history ops) are `SessionCommand`s;
  the mailbox's single consumer IS the per-session FIFO. There is no
  second session queue to keep consistent with it. Because every
  command is fast (§6.3), `turn/interrupt` is never stuck behind queued
  work.
- **Lifecycle order = the slot state machine (§7).** `session/archive`
  and `session/replace` are registry operations — a driver cannot
  arbitrate its own replacement, so lifecycle *decision and mutual
  exclusion* never live in the mailbox (the close path does deliver
  `SessionCommand::Close`, but only as the stop mechanism after the
  slot has already transitioned — §7.4). Mutual exclusion comes from
  slot transitions under the registry lock:
  `replace` requires `Live`; `archive` atomically moves
  `Live → Closing`; a concurrent lifecycle op on the same id observes
  the changed slot and fails or awaits accordingly (§7.3).
- **Two real auxiliary queues:**
  - `McpOauth(credential_key)` — OAuth refresh keyed like MCP
    credentials (definition site, §16.4), so a long-running turn cannot
    stall token refresh, and refresh bursts serialize per definition
    across its instances.
  - `ProcessConfig` — process-global config writes and plugin/config
    reloads; swap-snapshot only (§6.1); never held while awaiting
    session work.

Mailbox FIFO delivery and the lifecycle exclusions are part of the
contract (unit-tested).

Subagents do NOT enter the session mailbox — they execute inside the
parent turn under the `StreamingToolExecutor` safe-concurrent /
unsafe-serial model. Long-lived background agents and teammate runner
loops register their `JoinHandle` in `task_set` and descend their tokens
from `session_token`; cascade step 6 drains them.

## 10. Event Routing, Envelope, and Resume

### 10.1 SessionEnvelope — single stamping site

`CoreEvent` (3-layer Protocol/Stream/Tui) stays unchanged in `coco-types`.
AppServer owns the receiving end of every session's event sink and wraps
each event at one seam:

```rust
pub struct SessionEnvelope {
    pub session_id: SessionId,
    pub agent_id: Option<AgentId>,     // subagent attribution
    pub turn_id: Option<TurnId>,       // set for turn-scoped events
    pub session_seq: Option<i64>,      // durable events only, per-session monotonic
                                       // (i64 per the workspace integer convention)
    pub event: CoreEvent,              // Tui layer dropped by remote adapters
}
```

The router stamps `session_id` (it knows which sink fired — emitters
cannot forget or lie), copies `agent_id`/`turn_id` from the payload where
present, and assigns `session_seq` to durable events. One write site,
mirroring the "single `ProviderOptions` write site" convention. The
per-variant `session_id` fields on history notifications (`event.rs:379`
…) are subsumed by the envelope and removed.

**Durable vs ephemeral.** Not every event is worth replaying, and a seq
on a non-replayable event is a hole a reconnecting client stalls on. Two
classes, decided at the stamping seam:

- **Durable** — `Protocol`-layer notifications and boundary events
  (turn started/completed, item completed, history mutations, permission
  prompts, queue state, MCP state). Durable events get the next
  `session_seq`, enter the retention ring (§10.3), and are what the Hub
  stores (§13).
- **Ephemeral** — `Stream`-layer deltas (`TextDelta`, `ThinkingDelta`,
  tool progress) and the `Tui` layer. Delivered live to subscribed
  surfaces with `session_seq: None`, never retained, never replayed. A
  reconnecting surface reconstructs in-flight output from the snapshot
  plus the next boundary event, then follows new deltas live.

This mirrors opencode (durable events in SQLite with per-aggregate seq;
the live pubsub unsequenced) and keeps the Hub honest: everything
sequenced is persistable, so the durable stream has no seq holes within
a process epoch (crash recovery skips ahead — §10.3; every replay path
is `seq > cursor`, never contiguity-based, so epoch holes are
harmless). It also keeps the ring meaningful — 1024 *durable* events
is real history, where 1024 deltas is a fraction of one long response
(a single turn would wrap the ring and defeat replay).

### 10.2 Routing topology

```
SurfaceId  → SessionId              (forward, 1-to-1 per surface)
SessionId  → HashSet<SurfaceId>     (reverse fan-out)
SurfaceId  → ConnectionKey          (delivery)
ConnectionKey → HashSet<SurfaceId>  (cleanup on transport close)
```

All four maps and the per-session retention rings (§10.3) live under
the single `RoutingState` lock (§7.5 lock taxonomy) — cheap O(1)
mutations and `try_send`s, one lock, fixed order after the registry
lock.

One fan-out task per session (codex's listener-per-thread shape): it
reads the session's envelope stream and, per envelope, in ONE
`RoutingState` lock section, appends to the retention ring (§10.3) and
`try_send`s to each subscribed surface's outbound queue, honoring
per-surface notification preferences. `session/subscribe` is the mirror
image: its replay-read from the ring and the surface registration are
also one lock section. This pairing is the replay→live atomicity
mechanism — a subscriber either finds an envelope in its ring copy or
is registered before that envelope's delivery section; no gap, no
duplicate.

**Slow-consumer policy:** each connection's outbound queue is bounded
(default 1024 frames, §17). `try_send`; on full, disconnect that
connection (same path as transport close). Never block the emitter or
other surfaces.

**Transport close of `ConnectionKey K`:** remove K's surfaces from forward
and reverse maps, cancel K's pending server requests, other surfaces on
the same sessions continue, sessions do NOT end. `Disconnected` is
synthesized client-side by the SDK (§14) — the server never emits it (its
outbound queue is often already dead when the transport drops).

### 10.3 Reconnect and replay — snapshot + per-session seq

No retained-cursor invention: every reference product that works resumes
with *state snapshot + per-session sequence replay* (opencode) or plain
snapshot (codex, jcode). Adopted design:

- Each session keeps a bounded in-memory ring of recent **durable**
  `SessionEnvelope`s (§10.1; default 1024, §17), indexed by
  `session_seq`.
- **Restart continuity:** `session_seq` survives the process. The
  session's seq high-water mark is persisted as transcript metadata —
  written with the periodic transcript flush and at close cascade step
  5 — and on resume the counter is initialized to
  `watermark + event_retention_per_session` (**skip-ahead**). Without
  the watermark, a restarted process would re-issue seq 1, 2, 3 … under
  a Hub cursor that already reads 42; and because the watermark is
  flush-periodic, a crash can lose its tail — seqs already shipped to
  the hub above the persisted value. Skip-ahead makes re-issue
  impossible in both cases at the cost of a benign hole: replay is
  `seq > cursor` everywhere, never contiguity-based, and holes only
  occur across process epochs (§10.1). The hub additionally rejects a
  per-session seq regression as corruption. Seqs that fell out of the
  (empty) ring at restart degrade to `snapshot_required`, which is
  already the cold path.
- `session/subscribe { session_id, after_seq: Option<i64> }`:
  - `after_seq` within the ring → replay `> after_seq`, then live.
  - `after_seq` older than the ring (or absent) → the reply says
    `snapshot_required`; the client re-baselines via `session/read`
    (which returns the current `session_seq` high-water mark) and
    re-subscribes from there.
- The same `session_seq` domain is what the Hub speaks (§13), so live
  resume (ring) and durable replay (hub `EventStore` over JSONL) compose
  without a second cursor scheme.

### 10.4 Observability

Spans under a session carry `session_id` (and `agent_id`) as standard
fields; each runtime owns a root `session_span`, and spawned tasks attach
via `.instrument`. Envelope stamping is the natural place for an
events-emitted counter per `(session_id, layer)`.

## 11. Surface and Client Model

### 11.1 Connection vs surface

A **connection** is a transport relationship (client process, browser tab,
desktop app, gateway) keyed internally by `ConnectionKey`. A **surface** is
a user-visible attachment of that connection to one session. One
connection hosts any number of surfaces (desktop main chat + notification
strip; web multi-tab). This reverses v5 decision #6 deliberately — the SDK
contract is rewritten to match (§14).

### 11.2 SurfaceAttachment

Owned by AppServer (never by the session runtime):

```rust
struct SurfaceAttachment {
    surface_id: SurfaceId,
    connection: ConnectionKey,
    session_id: SessionId,
    role: SurfaceRole,                    // Interactive | Passive
    capabilities: SurfaceCapabilities,    // declared at attach
    notification_prefs: NotificationPrefs,
    last_delivered_seq: i64,
    state: SurfaceState,                  // Attached | SessionClosed
}

enum SurfaceRole { Interactive, Passive }
```

Clients own their local UI state (viewport, scroll, draft input). The
server owns canonical session state, the retention ring, and routing
metadata. jcode's `client_has_local_history`-style hints are unnecessary:
re-baselining is always `session/read` + seq.

### 11.3 Interactive ownership

Exactly one `Interactive` surface per session. A second interactive attach
is rejected with a typed, actionable error (jcode-informed shape — enough
information for the client to offer "take over" UX later without a retry
loop):

```rust
#[derive(Debug, Clone, Snafu)]
enum AttachError {
    #[snafu(display("session {session_id} already has an interactive surface"))]
    InteractiveOwnerConflict {
        session_id: SessionId,
        owner_surface: SurfaceId,
        owner_attached_at: DateTime<Utc>,
        owner_idle: bool,
    },
    #[snafu(display("surface limit reached for connection"))]
    SurfaceLimit,
    #[snafu(display("session is closing"))]
    SessionClosing,
}
```

Passive surfaces attach freely up to the per-session limit (§17) and never
gain input control. Interactive **takeover** (evict the current owner,
transfer in-flight approval routing) is v2; the v1 error carries the
fields takeover will need.

### 11.4 Capabilities are per surface

Declared at attach (not at initialize, not per connection). Server-
initiated requests that need a capability (file picker, keychain,
attestation, notifications) are sent only to a surface that declared it.
This fixes the codex per-connection capability scoping problem on shared
threads (their `request_processors/initialize_processor.rs:63` TODO) at
the design level.

### 11.5 Replace and archive routing for multi-surface

When `session/replace(old→new)` commits (§7.5 Stage 2):

- The **calling surface** is re-pointed old→new inside the commit section;
  it observes `session/started(new)` and continues seamlessly.
- **Other surfaces** on old receive `session/replaced { old, new }` and
  are detached from old's fan-out. They are NOT auto-attached to new —
  a dashboard tracking many sessions may not want to follow. Following is
  an explicit re-attach to `new`.

`session/archive(X)` is analogous: surfaces receive `session/ended(X)`,
their attachments move to `SurfaceState::SessionClosed`, and the client
dismisses or re-attaches elsewhere. Pending server requests for those
surfaces are cancelled (§8.3).

## 12. Protocol Adapters and Transports

### 12.1 LocalClientAdapter (v1 — TUI, headless)

Typed in-process calls and channels; zero JSON on the hot path. It does
not pretend to be a remote transport, but it goes through the same
AppServer ownership, routing, lifecycle, and subscription logic — the TUI
is a client, not a privileged co-owner.

### 12.2 JsonRpcAdapter (v1 — SDK stdio, UDS; WS behind a flag)

Maps JSON-RPC frames to canonical requests and back; request/response
correlation lives here, outside the session runtime.

### 12.3 WebAdapter (phase 5)

Projects AppServer state into browser APIs: `session/read` snapshots,
`session/subscribe` with `after_seq`, per-surface capability negotiation.
It never reads JSONL directly.

### 12.4 DesktopAdapter (phase 5)

UDS (Unix) / named pipes (Windows) by default. Desktop capabilities
(attestation, notifications, file picker, keychain) are surface-declared
(§11.4); desktop-only server requests are gated on declaration.

### 12.5 ImGatewayAdapter (phase 5)

Converts platform events into canonical session input and back. The
gateway owns:

- the durable **`channel key → SessionId` map** (the OpenClaw two-level
  identity, §3): a stable per-(platform, channel, user, thread) key whose
  target SessionId is re-pointed by replace — AppServer never learns
  platform keys;
- platform credentials, tokens, callback capabilities (Hermes model:
  capability vault gateway-side, token-less references only);
- channel capability declarations, rate limits, allowlists.

An IM `/stop` resolves through the gateway's map to one targeted
`turn/interrupt { session_id, turn_id }`; it cannot touch other sessions
on the same gateway connection.

### 12.6 Transport trait

Connection acceptance, framing, backpressure, close detection, write
flushing. Transports never inspect transcripts, own sessions, or
implement turn semantics.

## 13. Hub Integration

The Hub is the durable, read-side projection; AppServer is the live
routing side. They share one cursor domain.

- Hub envelopes become per-session sequenced:
  `{ instance_id, session_id, agent_id?, session_seq, ts, schema_version, payload }`.
  `session_seq` is the SAME counter stamped by AppServer routing (§10.1).
  Only durable events are sequenced and shipped (§10.1), so the hub's
  stored stream has no seq holes within a process epoch; crash recovery
  skips ahead (§10.3) and replay is `seq > cursor`, so epoch holes are
  harmless. Because the watermark (plus skip-ahead) persists in
  transcript metadata, hub cursors remain valid across AppServer
  restarts; the hub rejects a per-session seq regression as corruption
  rather than storing it.
- The current `hub/protocol` v1 frames cannot express this
  (`AnnounceAckFrame.resume_from: Option<u64>` single cursor,
  `hub/protocol/src/lib.rs:39`; scalar `BatchAckFrame.up_to_seq`, `:51`;
  per-instance global `seq` on `EventEnvelope`, `:63`). Since backward
  compatibility is a non-goal, **v1 frames are deleted, not retained**:
  the subprotocol becomes `coco-event-hub.v2` with per-session cursors.

```rust
pub const SUBPROTOCOL_V2: &str = "coco-event-hub.v2";

#[serde(rename_all = "camelCase")]
pub struct AnnounceAckFrameV2 {
    pub first_seen: bool,
    pub hub_version: String,
    /// Per-session resume cursors, scoped to the sessions listed in
    /// the announce frame (the instance's live set); missing key = hub
    /// has nothing yet. Never a dump of every session ever stored —
    /// that map is unbounded for a long-lived install.
    pub resume_from: HashMap<SessionId, i64>,
}

#[serde(rename_all = "camelCase")]
pub struct BatchAckFrameV2 {
    /// Per-session high-water-mark of durably stored seqs.
    pub up_to_seq: HashMap<SessionId, i64>,
}
```

- Hub `EventStore` indexes `(session_id, session_seq)`; replay is
  `events_for_session(x, seq > resume_from[x])`, O(per-session) via the
  composite index. Cross-session ordering is intentionally undefined.
- No `thread_id` anywhere in Hub protocol, storage, routes, or rows.
- **`event-hub/spec.md` §4 must be revised in the same change**: its
  "one live session per process, id rotates on `/clear`" identity section
  contradicts this design and the citation of the rotate-in-place site
  goes away with the demolition list (§18).

## 14. SDK Client Contract

`coco-app-server-client` (Tier 2) replaces the v5 1-client-1-session
binding with a two-level API. The type system enforces the identity rule:
a session client is consumed by the operations that end its session.

```rust
pub struct ServerClient { /* one connection */ }

impl ServerClient {
    pub async fn connect(opts: ConnectOptions) -> Result<ServerClient, ClientError>;
    pub async fn start_session(&self, params: SessionStartParams)
        -> Result<SessionClient, ClientError>;
    pub async fn resume_session(&self, id: SessionId, params: SessionResumeParams)
        -> Result<SessionClient, ClientError>;
    pub async fn list_sessions(&self, q: SessionListQuery)
        -> Result<Page<SessionSummary>, ClientError>;
    pub async fn close(self) -> Result<(), ClientError>;   // connection only
}

pub struct SessionClient { /* one interactive surface on one session */ }

impl SessionClient {
    pub fn session_id(&self) -> &SessionId;                 // capture before disconnect risk
    pub async fn query(&self, input: TurnInput) -> Result<TurnId, ClientError>;
    pub async fn interrupt(&self) -> Result<(), ClientError>;
    pub fn events(&mut self) -> impl Stream<Item = SessionEnvelope> + '_;
    /// `/clear`. Consumes self; returns the handle on failure.
    pub async fn replace(self) -> Result<SessionClient, (SessionClient, ClientError)>;
    /// `session/archive`. Consumes self; returns the handle on failure.
    pub async fn close(self) -> Result<(), (SessionClient, ClientError)>;
}

pub struct PassiveSessionClient { /* one passive surface */ }

impl PassiveSessionClient {
    pub fn session_id(&self) -> &SessionId;
    pub fn events(&mut self) -> impl Stream<Item = SessionEnvelope> + '_;
    pub async fn read(&self, q: SessionReadQuery) -> Result<SessionSnapshot, ClientError>;
    pub async fn detach(self) -> Result<(), ClientError>;
}
```

- `replace(self)` / `close(self)` consume the handle — a `SessionClient`
  can never be re-pointed to a different `SessionId`, mirroring §6.4. On
  failure they return the original handle in the error
  (`Err((self, e))`): a failed replace (`ConstructFailed`) or a
  timed-out close leaves the session alive server-side, and swallowing
  the handle would orphan it (§7.6). Retry or recover with the same
  handle.
- `start_session` / `resume_session` attach an `Interactive` surface and
  return `SessionClient`. Passive observation is a different type:
  `subscribe_session(id) -> PassiveSessionClient` has no
  `query`/`interrupt`/`replace`, so the one-interactive-owner rule
  (§11.3) is enforced at compile time for SDK users, not only at the
  server.
- `events(&mut self)` borrows the client's single underlying receiver:
  one live stream at a time (a second concurrent stream is a compile
  error), successive calls continue the same queue, and every envelope
  is delivered exactly once per client. Fan-out to multiple consumers
  is a client-side concern.
- Sequential sessions on one connection and concurrent sessions on one
  connection are both just multiple `SessionClient`s /
  `PassiveSessionClient`s.

**Errors and disconnect (dual-channel, kept from v5):**

```rust
#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("connection failed: {0}")]      Connect(String),
    #[error("transport disconnected")]       Disconnected,
    #[error("client invalid (reconnect and resume)")] ClientInvalid,
    #[error("server error {code}: {message}")] Server { code: i32, message: String },
    #[error("request timed out")]            Timeout,
    #[error("invalid argument: {0}")]        InvalidArgument(String),
}
```

On transport close the SDK transport task (1) resolves every in-flight
RPC future with `Err(Disconnected)` and (2) synthesizes a terminal
`Disconnected` item on every `events()` stream — RPC-only would leave
stream consumers hanging; event-only would leak awaiting futures. After
that, every call on the `ServerClient` and all its `SessionClient`s
returns `ClientInvalid`. No auto-reconnect; recovery is user-owned:

```rust
let saved = session.session_id().clone();
// ... disconnect ...
let server = ServerClient::connect(opts).await?;
let session = server.resume_session(saved, params).await?;
```

`Drop` is silent for both types; `close()` is the explicit cleanup.

## 15. TUI Behavior Contract (v1)

- Startup → `ServerClient::connect` (LocalTransport) →
  `start_session(cwd, …)`; TUI holds exactly one `SessionClient`.
- `/clear` → `session.replace()` (atomic §7.5; the TUI swaps its handle).
- `/resume <id>` → `session.close().await` (archive current — without it,
  repeated `/resume` accumulates live sessions until `max_sessions`) →
  `resume_session(id)`.
- `/quit` → `session.close()` → exit.
- Multi-window / multi-session TUI is out of scope; evolving to
  `HashMap<SessionId, SessionClient>` requires zero protocol change.

## 16. MCP Configuration and Isolation

### 16.1 Definition sources

MCP server definitions follow the same three scopes as configuration
(§6):

- **User catalog** (ProcessRuntime): servers from `~/.coco` settings —
  candidates for every session.
- **Project catalog** (ProjectServices): `.mcp.json` + project-settings
  contributions — candidates only for sessions of that project, gated by
  per-project approval stored in user-level state keyed by project root
  (same pattern as project trust, §6.2).
- **Session**: the fold at `session/start` (§6.5) computes the session's
  effective server set — user catalog ∪ project catalog, the project
  definition winning on a name collision (standard more-local-wins
  layering).

### 16.2 Instance scope

`McpScope::{Shared, PerSession}` per server, configured in
`mcp_servers[].scope`. **`Shared` shares at the scope of the defining
layer** — "one process-wide instance" is only correct for user-defined
servers:

| Defined in | `Shared` instance | `PerSession` instance |
|---|---|---|
| user catalog | one per process; spawn cwd = coco home; must not depend on any project cwd (requests carry session context) | one per root session; spawn cwd = session cwd |
| project catalog | one per project root, keyed `(project_root, server_name)`; spawn cwd = project root; shared by that project's sessions; torn down with the `ProjectServices` entry | one per root session; spawn cwd = session cwd |

Without the project keying, a project-defined "shared" server would leak
across projects and run with the wrong cwd — the same bug class the
per-session config fold exists to prevent (§6.5).

### 16.3 Child lifecycle

- Spawn hardening: `PR_SET_PDEATHSIG(SIGTERM)` on Linux / kqueue
  parent-watch on macOS.
- PID files (validated `McpServerName` newtype, same path-safety rules
  as `SessionId`): per-session instances at
  `<session_id>/mcp-pids/<server_name>.pid`; per-project instances at
  `projects/<slug>/mcp-pids/<server_name>.pid`.
- Teardown: close cascade step 4 (SIGTERM → 5 s grace → SIGKILL) for
  per-session instances; `ProjectServices` eviction runs the same
  sequence for per-project instances; process startup sweeps both
  `mcp-pids/` layouts and reaps orphans.

### 16.4 Credentials

OAuth credentials are keyed by the **definition site**, not by instance:
a user-defined server has one credential set per server name,
process-wide; a project-defined server's credentials are keyed
`(project_root, server_name)` — two projects defining unrelated servers
under the same name never share tokens. All instances of one definition
(including every `PerSession` child) share its credentials —
`PerSession` does NOT mean per-session OAuth identity. Refresh
serializes through the `McpOauth` queue keyed the same way as the
credentials; burst contention on simultaneous expiry is accepted for
v1.

## 17. Configuration

All keys under `server.*` in `~/.coco/config.json`; env via
`coco_config::EnvKey` variants (never ad-hoc `std::env::var`); CLI flags
take precedence per the standard resolution order.

| Key | Default | Notes |
|---|---|---|
| `server.max_sessions` | 32 | over limit → `ResourceExhausted`; no eviction; `replace` bypasses by +1 (§7.5) |
| `server.event_retention_per_session` | 1024 envelopes | ring size backing `after_seq` replay (§10.3) |
| `server.outbound_queue_frames` | 1024 | per connection; full → disconnect (§10.2) |
| `server.max_surfaces_per_connection` | 8 | typed `SurfaceLimit` error |
| `server.max_passive_surfaces_per_session` | 16 | resource guard for fan-out |
| `server.project_services_idle_ttl_secs` | 3600 | evict `ProjectServices` entries with zero attached sessions (§6.2) |
| `server.idle_session_timeout_secs` | off | optional auto-archive of sessions with zero surfaces and no active/queued turn (§7.6) |
| `server.turn_drain_timeout_secs` | 10 | close cascade step 3 bound; on timeout the cascade proceeds to the abort backstop (§7.4) |
| `server.shutdown_timeout_secs` | 30 | process-shutdown drain bound (§7.7) |

Ownership recap (three scopes, §6):

- **Process**: policy/user/flag/env settings layers, `CatalogPaths`,
  model factories + registry, auth/keyring, user-level MCP catalog,
  settings persistence (`settings.json` writes), transports,
  `SessionManager`.
- **Project** (per project root, cached): project + local settings
  layers, project permission rules, hooks, skills/commands, `.mcp.json`
  contributions, CLAUDE.md discovery, ignore service, LSP, retrieval.
- **Session**: the folded `Arc<RuntimeConfig>` snapshot (including the
  resolved `Arc<Features>`), cwd, history, `ToolAppState`, turns, inbox,
  memory, usage, MCP runtime handles, session permission rules,
  `worktree_state`.

Worktree remains gated by `Feature::Worktree` as resolved in the
session's fold; there is no separate `session/start` parameter.

## 18. Migration Plan — build parallel, cut over once

No dual-stack, no compatibility shims, no transitional protocol variants.

**Phase A — foundations (independent, parallelizable PRs):**

1. Typed identity adoption in `coco-types` + call sites: `SessionId`
   everywhere (kill `String`/`Uuid` duplicates), `TurnId` in
   `AgentStreamEvent`/`ThreadItem`, new `SurfaceId`.
2. `SessionEnvelope` in `coco-types`; strip per-variant `session_id`
   fields from history notifications.
3. `coco-app-runtime`: extract `ProcessRuntime` / `SessionRuntime` /
   `SessionHandle` from `session_runtime.rs` (the `:549` comment becomes
   the crate boundary).
4. `coco-app-server`: registry (§7), serialization model (§9), routing + ring (§10),
   surfaces (§11 minus takeover), `LocalClientAdapter`, `JsonRpcAdapter`.
5. `coco-app-server-transport`, `coco-app-server-client` (§14).
6. Hub v2: `coco-event-hub.v2` frames, `(session_id, session_seq)`
   EventStore index, connector egress off the envelope stream; revise
   `event-hub/spec.md` §4.
7. `ProjectServices` + `ProjectRegistry` (§6.2): move project/local
   settings, project permission rules, hooks, skills, `.mcp.json`,
   CLAUDE.md discovery, ignore, LSP, retrieval behind the per-project
   container; move the `build_runtime_config_with` call from process
   boot to `session/start` (§6.5).
8. cwd discipline (§6.5): delete the `std::env::current_dir()`
   fallbacks on session-scoped paths, add the `clippy.toml`
   `disallowed-methods` entry, thread cwd through
   `ToolUseContext`/spawn sites.

Implementation progress as of 2026-07-07:

- `TurnId` is now typed on `AgentStreamEvent` deltas and `ThreadItem`; its
  inner string is private, so callers construct through `TurnId::from` /
  `TurnId::generate` instead of tuple-field access. Protocol
  `ServerNotification` turn-id-bearing payloads in `common/types/src/event.rs`
  now also store `TurnId` internally while preserving the same string-shaped
  serde wire format. Query stream, model-fallback notice, and MoA reference
  emission paths now carry typed `TurnId`s through their internal helper
  signatures instead of passing raw strings through stream events or reusing
  session id as a turn id. SDK `TurnStartResult`, session-trace
  `TraceEvent` turn boundaries, and wire-dump `WireTurnCtx` / `WireRecord`
  now also store `TurnId` internally while preserving string-shaped JSON.
- `SurfaceId`, `SessionEnvelope`, and the durable/ephemeral replay
  taxonomy are in `coco-types`.
- `coco-app-server` now exists as the first app-server foundation crate.
  It owns the private `ConnectionKey`, `RoutingState`'s connection/surface
  indexes, per-session durable retention rings, `after_seq` replay vs
  `snapshot_required`, live-only ephemeral delivery, transport-close cleanup,
  and slow-consumer disconnect behavior. It also now owns
  `SurfaceAttachment`, `SurfaceRole`, per-surface capabilities and
  notification preferences, per-connection/passive-session surface limits, and
  v1 interactive-owner enforcement with typed `InteractiveOwnerConflict`
  metadata. Its `RoutingState` also now has the §11.5 surface half of
  replace/archive: `replace_calling_surface` re-points only the caller to the
  replacement session and closes old peer surfaces, while `archive_session`
  moves all live surfaces on that session to `SurfaceState::SessionClosed` and
  removes them from fan-out. It also owns the §8.3/§11.4 pending
  server-request bookkeeping foundation: server-minted monotonic request ids,
  capability-gated routing to the interactive surface, `(request, session,
  surface, turn)` indexes, session validation on completion, and precise
  cancellation for connection close, surface close/detach, turn transition,
  replace, and archive. It now also has an in-memory actionable request channel
  foundation: connections can register a separate `ServerRequestDelivery`
  sender, and `route_server_request` records pending ownership then `try_send`s
  the request to the target interactive surface's request channel without
  mixing it into the session-envelope stream. Routed actionable request
  payloads are retained only while their pending ownership is open, and
  `pending_server_request_replays_for_surface` exposes the retained request plus
  pending metadata for in-memory late-attach replay; completion or cancellation
  clears both the pending indexes and retained payload.
  `AppServer::resolve_server_request` now validates client replies against
  pending `(session_id, request_id)` ownership, clears the pending indexes, and
  returns the reply payload for the future runtime/adapter bridge. AppServer
  now also exposes adapter-facing request routing and pending-request replay
  wrappers, so request delivery, replay lookup, and reply completion are owned
  at the same layer even before concrete transports land.
  `LiveSessionRegistry` now also exists as the §7
  slot state skeleton: `Loading`, `Live`, and `Closing(handle, completion)` all
  count toward `max_sessions`; concurrent callers observe cloneable load/close
  completion signals; load success promotes to `Live`, load failure removes the
  slot, and close completion removes `Closing`. Its registry-only replace
  skeleton now covers §7.5 Stage 1 and the registry half of Stage 2:
  `begin_replace` requires old `Live` and reserves the new id as `Loading`
  while bypassing `max_sessions` by exactly one swap slot;
  `complete_replace_success` promotes new `Loading → Live` and marks old
  `Live → Closing` in one no-await write-lock section; construction failure
  removes only the new slot and leaves old live. `AppServer::spawn_load` now
  provides the §7.2 load owner-task entry point: the caller that reserves a
  fresh `Loading` slot spawns the factory future and later callers observe the
  same completion signal without running duplicate factories.
  `AppServer::spawn_close` now provides the live-session close owner-task entry
  point: it marks `Live → Closing`, runs the supplied close cascade future in a
  spawned task, then completes archive routing and removes the slot even if the
  origin caller drops its completion signal. Close-on-`Loading` now records a
  close-after-load request in the slot: load failure completes the close signal
  immediately, while load success moves directly into `Closing` and lets the
  single close owner task run the supplied cascade. `AppServer::spawn_replace`
  now provides the §7.5 replace owner-task entry point: it reserves the
  replacement as `Loading`, runs the construction future, commits the
  registry+routing swap on success, then runs the supplied old-session close
  cascade and archive completion; construction failure removes only the
  replacement slot and leaves old live. These owner tasks now also route
  lifecycle effects through the in-memory lifecycle delivery channel after
  commit locks are released: replace emits started/replaced before the old close
  cascade, and close/archive emits ended after archive commit. `AppServer` now owns the first
  combined registry+routing commit skeleton:
  `commit_replace_for_surface` takes the registry lock before the routing lock,
  validates that the calling surface is still attached to old, commits new
  `Loading → Live` plus old `Live → Closing`, then re-points the caller surface
  old→new and closes old peers through `RoutingState` before releasing the
  locks. The close/archive supervisor-completion commit skeleton is also present:
  `complete_close_and_archive_surfaces` requires a `Closing` registry slot,
  takes registry then routing locks, moves the session's surfaces to
  `SurfaceState::SessionClosed`, completes the close signal, and removes the
  registry slot. These commit methods now also return internal
  `SurfaceLifecycleEffect`s identifying which surfaces should receive
  `SessionStarted`, `SessionReplaced`, or `SessionEnded` after the locks are
  released. `RoutingState` now also has an in-memory lifecycle delivery
  channel: connections can register a separate lifecycle sender, and adapters
  can route those post-commit effects to the target surfaces after the commit
  locks are released, including surfaces just moved to `SessionClosed`.
  `LocalClientAdapter` now exists as the first typed in-process adapter
  skeleton: it registers a real AppServer connection with separate event,
  server-request, and lifecycle channels, then attaches/subscribes surfaces
  through the same AppServer routing rules future transports will use. It also
  exposes a connection-scoped surface detach path, so passive/local clients can
  drop one surface without closing the connection or archiving the session.
  `JsonRpcAdapter` now exists as the remote adapter foundation: it registers
  real AppServer connections with the same event, server-request, and lifecycle
  channels, converts `ServerRequestDelivery` payloads into JSON-RPC request
  frames, and owns `JsonRpcId -> (SurfaceId, RequestId)` response correlation
  for server-initiated requests. It deliberately does not dispatch
  runtime-backed `session/*` / `turn/*` methods yet.
  AppServer now also exposes a live-session summary projection that combines
  registry live slots with routing surface counts, covering the live half of
  the `session/list` surface-count contract while leaving persisted transcript
  summaries to the future runtime/session-store bridge. The
  `coco-app-server-client` crate now exists as the first client-side
  foundation slice: it depends on `coco-app-server`, exposes a local
  in-process `ServerClient` over `LocalClientAdapter`, returns distinct
  `SessionClient` and `PassiveSessionClient` handles with typed
  `SessionId`/`SurfaceId` accessors, and consumes passive handles when
  detaching one surface from a connection. Snapshot-required subscribes do not
  mint passive handles, preserving the §10.3 rule that a missing/too-old cursor
  must read a snapshot before attaching live. The local client now also demuxes
  the shared connection event, server-request, and lifecycle receivers by
  `SurfaceId` so reading one handle does not consume another handle's delivery;
  this is the in-process foundation for the future per-handle stream/request
  API. It also exposes a client-side live-session list projection with current
  surface counts, covering the live half of §14 `list_sessions` while persisted
  transcript metadata remains pending. `coco-app-server-transport` now exists
  as the pure wire-format foundation for remote transports: it owns JSON-RPC
  frame/id/error response serde, preserves arbitrary JSON params/result/data,
  and deliberately has no dependency on `coco-app-server`. It also provides the
  first NDJSON per-record codec with LF/CRLF decode, trailing-newline encode,
  and max-frame rejection, plus generic async NDJSON reader/writer primitives
  over caller-owned streams. It now also has a generic NDJSON duplex connection
  wrapper that tracks local open/closed state and clean EOF without owning
  accept loops or AppServer cleanup, plus a process stdin/stdout constructor
  for the same framing layer. Connection-owner tasks, UDS/named-pipe,
  WebSocket framing, slow-consumer backpressure, full adapter-side close
  cleanup, and adapter integration are still pending. This establishes the §14
  two-level handle boundary before remote transports or runtime-backed
  start/resume operations land. The
  crate is intentionally not wired to `SessionRuntime`, TUI, SDK, or Hub yet;
  concrete runtime factory wiring behind `spawn_load`, concrete close cascade
  implementation behind `spawn_close`, concrete replace runtime factory and
  old-session close cascade behind `spawn_replace`, remote transport adapters,
  JSON-RPC method dispatch, transport-side server-request replay and typed
  reply plumbing beyond JSON-RPC response-id correlation, and wire mapping for
  lifecycle effects remain pending Phase A work.
- The staged compact ledger and `QueryEngine.staged_session_id` now use
  `SessionId` instead of `Uuid`.
- `QueryEngine.transcript_session_id` now stores `SessionId`; the
  transcript store remains a string path boundary.
- `QueryEngineConfig.session_id` now stores `SessionId`; protocol,
  transcript, and tool-runtime boundaries still convert explicitly to
  `String` / `&str`.
- SDK `session/resume`, `session/read`, and `session/archive` request params
  now carry typed `SessionId` in `coco-types` while preserving the same
  string-shaped JSON wire format; SDK handlers convert only at the legacy
  persistence API boundary.
- SDK `session/start` result and persisted-session response summaries now carry
  typed `SessionId` in `coco-types`; session list/read/resume convert from the
  legacy persistence string boundary, and the TUI session browser converts only
  at its string-backed picker state boundary.
- `SessionUsageSnapshot.session_id` now carries typed `SessionId`; usage
  accounting and cost snapshots receive typed runtime ids directly, while
  persisted `usage.json` keeps the same string-shaped serde wire format.
- Reserved remote-teammate task extras and persisted worktree-session state now
  carry typed `SessionId` in `coco-types`; both preserve their existing
  string-shaped serde wire format.
- History lifecycle protocol notifications (`MessageAppended`,
  `MessageTruncated`, `SessionResetForResume`, `HistoryReplaced`) now share a
  flattened `ServerNotificationIdentity` in `coco-types`; query/CLI
  constructors pass typed ids for true session envelopes and `None` for legacy
  empty-envelope paths, while serde still accepts missing / empty string
  session ids as `None` and keeps the existing string-shaped wire fields.
- Transcript persistence DTOs owned by `coco-types` (`SerializedMessage`,
  `TranscriptMessage`) now store `SessionId` internally while preserving the
  existing string-shaped JSON field and rejecting unsafe ids during serde
  decode; `common/types/src` no longer has bare `session_id: String` fields.
- Prompt-history persistence now stores the active session identity as typed
  `SessionId` in `PromptHistory` / `HistoryLogEntry`, with TUI hydration and
  prompt-save paths passing typed runtime ids and JSONL output remaining
  string-shaped.
- Session persistence core now stores session identity as typed `SessionId`
  across the PID registry (`SessionRegistration`, `PsEntry`), catalog
  resolution (`ResolvedSession`), transcript entries / metadata
  (`TranscriptEntry`, `MetadataEntry`, `TranscriptMetadata`), recovery, and
  in-memory/disk store folds. Path-oriented store traits still accept `&str`
  at legacy filesystem boundaries, but JSON/JSONL output remains string-shaped
  and `app/session/src` no longer has bare `session_id: String` fields.
- Durable job and trace sidecars now keep session identity typed internally:
  `coco_tasks::JobState.session_id` and
  `coco_session_trace::TraceManifest.session_id` use `SessionId` while their
  JSON files remain string-shaped.
- `coco_messages::MessageHistory` stores the transcript envelope as
  `Option<SessionId>` instead of an empty string sentinel; `history_sync`
  clones the typed id directly for lifecycle notifications. The legacy
  persisted `coco_messages::HistoryEntry.session_id` also uses `SessionId`.
- Protocol lifecycle payloads `SessionStartedParams` and `SessionResultParams`
  now carry typed `SessionId` in `coco-types` while preserving the same
  string-shaped JSON wire format; query and SDK event constructors clone typed
  runtime ids, and the TUI converts only at its string-backed UI state boundary.
- `ServerNotification` now exposes `session_id()` / `agent_id()` accessors as
  the migration seam toward `SessionEnvelope`: history notifications and
  lifecycle payloads can be consumed through one typed identity accessor, and
  the four history variants no longer duplicate per-variant Rust fields for
  `session_id` / `agent_id`.
- `QueryEngineConfig::workspace_cwd` centralizes session workspace
  resolution from `cwd_override` / `project_dir` / `original_cwd`.
  Query prompt construction and hook orchestration contexts now use it
  instead of live `std::env::current_dir()` fallbacks.
- `ToolUseContext::effective_shell_cwd` now resolves from
  `cwd_override` / live `session_cwd` / `original_cwd` before a fixed
  `/tmp` test fallback, and the query-layer canUseTool callback uses that
  helper instead of reading the process cwd directly.
- Query-layer runtime paths under `app/query/src` no longer call
  `std::env::current_dir()` directly; compaction attachments, turn
  reminders, dynamic attachments, transcript metadata, direct-edit memory
  detection, and memory-write detection now resolve from session workspace
  helpers. Remaining `app/query/src` current-dir reads are test-only.
- The fused `SessionRuntime` no longer calls `std::env::current_dir()`
  directly; hook contexts, file-watch hook factories, transcript metadata,
  local permission persistence, hook reload, and agent-catalog reload now
  use `QueryEngineConfig::workspace_cwd()` or the runtime's live
  `current_cwd`.
- TUI runtime-backed paths now resolve cwd from the session runtime for
  prompt-mode shell commands, plugin reload, turn input @-mentions, plugin
  dialog loading, agent creation, the permissions editor, `/context` memory
  path display, and agent-dialog create finalization. SDK handlers now share
  `SdkServerState::workspace_cwd()`, preferring the active SDK session cwd,
  then the installed runtime cwd, before the legacy no-runtime fallback.
- TUI app construction now receives the session cwd from `app/cli` and uses
  it for the shared file index and git-index watcher; `app/tui/src` no longer
  reads `std::env::current_dir()` in production code.
- Core tool implementations now use `ToolUseContext` cwd anchors for
  Glob, Grep, LSP, ApplyPatch, SendUserMessage attachments, skill-trigger
  tracking, and worktree creation. Synchronous permission/secret helpers use
  `cwd_override` / `original_cwd` rather than process cwd; Bash's
  context-free read-only trait fallback now uses a fixed fallback instead of
  process cwd, while permission evaluation continues to use explicit
  `shell_cwd`.
- Command handlers that need project-local state now receive the registered
  project root instead of reading process cwd: loop skill prompt rendering,
  `/init`, `/agents`, `/skills`, `/plugin enable|disable`, `/lsp`,
  `/stats`, `/commit`, `/commit-push-pr`, and the hidden `/env` diagnostic
  output. `commands` no longer has a non-test `std::env::current_dir()` read.
- Nested-memory phase-1 conditional rules now use the `traverse_for_file`
  cwd argument; workflow source resolution falls back to `"."` only when no
  cwd is supplied, and the Workflow tool now supplies `ToolUseContext`'s cwd
  anchor for execution and permission previews. File permission rule matching
  and shell suggestion helpers now use explicit cwd inputs instead of reading
  process cwd.
- Subagent sidechain transcript writes now stamp cwd from the session
  transcript adapter instead of `TranscriptStore` reading process cwd; hook
  command execution receives `HOOK_CWD` from `OrchestrationContext`, and
  marketplace local-path parsing has an explicit cwd-aware entry point used
  by `/plugin marketplace add`.
- Shell path validation's git-escape helper now delegates to the explicit-cwd
  variant with a fixed fallback, and Windows MCP program resolution uses the
  configured MCP server cwd instead of process cwd when resolving PATH entries.
  SDK no-runtime cwd resolution now falls back to the initialize bootstrap's
  captured cwd (then `"."`) instead of reading process cwd. Provider login
  resolution now receives an explicit cwd from the CLI entrypoint or the live
  TUI runtime, so configured OAuth provider lookup no longer reads process cwd.
  CLI bin handlers for `config`, `plugin`, `moa`, and `agents` now receive cwd
  from `main` instead of reading process cwd internally; plugin install/validate
  resolves relative paths against that explicit cwd. The interactive TUI runner
  and tracing initialization now also receive the startup cwd from `main`, and
  headless / SDK paths reuse that same snapshot. `app/cli` production
  `current_dir()` reads are down to the single `main` startup boundary plus
  the public `headless::run_chat` convenience fallback; the lower-level
  `run_chat_with_options` entrypoint now requires `RunChatOptions::cwd`
  unless the CLI carries `--cwd`.
- `scripts/check-session-cwd-discipline.sh` is wired into `just check-seam`
  to reject new process-cwd reads in session-owned production crates. It
  allow-lists only the CLI startup boundary and the documented
  `headless::run_chat` convenience capture; full-workspace `clippy.toml`
  enforcement remains the steady-state target after standalone utilities are
  split or allow-listed.
- `coco-file-search` no longer reads process cwd from its reusable
  `run_main` library entrypoint; the standalone binary now fills the cwd at
  its CLI boundary before delegating.
- `app/cli` now resolves a `SessionWorkspace` snapshot at runtime build time,
  separating the session cwd, the future `ProjectServices` cache key
  (`project_root` = git worktree root, else cwd), and the existing transcript /
  memory `ProjectPaths` storage anchor. `SessionRuntime` stores the project
  root and reuses the single storage-path snapshot instead of recomputing it
  for transcript and file-history setup. Cron loop expansion, loop-skill
  runtime context, and `/resume` same-project checks now consume the resolved
  project root instead of treating the original cwd as a project root. LSP
  manager construction, plugin-contributed LSP merge, and LSP reload/prewarm
  now also use the resolved project root rather than the session's current cwd.
- MCP config loading now has an explicit roots split: project-scoped files
  (`.mcp.json` and `.coco/mcp.json`) and plugin-contributed MCP servers can be
  loaded against the resolved project root, while `.coco.local/mcp.json` remains
  session-cwd scoped. Session MCP bootstrap uses that split, preserving local
  override priority while preparing the project catalog for `ProjectServices`.
  Shell `coco mcp login/logout` now receives cwd from the CLI boundary and
  uses the same project-root/session-cwd split when resolving OAuth-capable
  MCP server definitions.
- Session bootstrap and plugin reload now load the enabled plugin catalog from
  the resolved project root. Plugin-contributed commands, skills, hooks, output
  styles, LSP servers, and MCP servers therefore follow the project catalog
  boundary, while disk skill discovery still uses the session cwd walk so nested
  `.coco/skills` behavior is preserved until the project/local settings split
  is implemented. Plugin-contributed agent directories and the initial
  session hook registry's plugin layer now use the same project-root plugin
  catalog; direct disk agent discovery still starts from the session cwd. The
  project plugin catalog is now represented by `ProjectCatalogSnapshot` and
  exposed through a thin `ProjectServices` wrapper. `EngineResources`,
  `SessionRuntimeBuildOpts`, and `SessionRuntime` now carry
  `Arc<ProjectServices>`, and `app/cli::project_services::ProjectRegistry`
  caches those handles per `(config_home, project_root)` until `ProcessRuntime`
  owns the registry field. Startup paths share the cached entry; explicit
  plugin/LSP/MCP/hook reload paths force-refresh the entry so live reload still
  sees newly enabled, disabled, installed, or removed plugins. Idle eviction,
  attached-session ref-counting, and the full project/local settings split are
  still pending.
- `UsageAccounting` now owns its mutable session id as `SessionId`; it no
  longer shares the runtime identity lock. It exposes lifecycle-level
  retarget methods for loading an existing session's usage or starting a
  fresh empty usage ledger, so `SessionRuntime` no longer sequences raw
  tracker load/reset operations. Usage snapshot load/read/flush now go
  through `UsageAccounting`, and `SessionRuntime` no longer stores duplicate
  usage tracker/write-lock handles, mutates the tracker lock directly, or
  owns the side-query usage event sink slot. String conversion happens only
  at transcript / snapshot store read/write boundaries.
- `QueryEngine` usage recording and stop-hook usage reads now go through
  `UsageAccounting`; the old tracker/write-lock builder compatibility fields
  and methods have been removed, along with the old usage-accounting
  tracker/write-lock accessors and event-sink builder injection point.
- Production `UsageAccounting` construction now creates its own tracker/write
  lock; shared tracker injection remains only behind the test-only
  `for_static_session` helper for narrow tests that assert cumulative totals
  directly.
- Normal runtime session-id reads now snapshot typed `SessionId` from
  `QueryEngineConfig.session_id`; detached hook orchestration also snapshots
  session identity from the synchronized engine config mirror. File-history
  snapshot persistence now uses that same synchronized engine config mirror,
  so the separate `session_identity.rs` helper module and file-history
  session-id mirror are gone.
- The old `SessionRuntime::adopt_session_id` aggregation method is gone;
  legacy identity retargeting now lives in `retarget_session_id_boundaries`,
  while runtime-owned session-keyed state reset lives in the unified
  `retarget_runtime_for_session` seam.
- The `/clear` flow now separates pre-retarget conversation cleanup
  (`reset_conversation_state_before_clear_retarget`) from the shared
  empty-session retarget seam.
- Post-clear SessionStart hook construction and execution now lives behind
  `run_session_start_hooks_after_clear`, leaving `clear_conversation` as a
  five-step orchestration path.
- The `/clear` orchestration path now lives in the dedicated
  `session_runtime::clear` child module, keeping pre-clear rewind capture,
  conversation-local reset, empty-session retarget, and post-clear
  SessionStart hooks isolated from plugin/hook reload and per-turn engine
  wiring in the main runtime body.
- Session hook orchestration now lives in the dedicated
  `session_runtime::hooks` child module: SessionStart/End, Setup,
  UserPromptSubmit, Notification, CwdChanged, ConfigChange, and FileChanged
  watch registration are separated from the main runtime body while keeping
  the existing public methods. The FileChanged watcher registration context
  and async-hook rewake sink helper now live there too. Hook registry bootstrap
  population and ConfigChange source mapping now live there as well, leaving
  the parent module free of hook-runtime helper definitions.
- Agent catalog snapshot/reload management now lives in the dedicated
  `session_runtime::agent_catalog` child module, including SDK-supplied
  agent injection and live catalog handles, while preserving the existing
  `SessionRuntime` public methods.
- Plugin, command, hook, MCP, and LSP reload paths now live in the dedicated
  `session_runtime::reload` child module, keeping project-service refreshes
  and live registry swaps out of the main runtime body while preserving the
  existing `SessionRuntime` public entrypoints.
- Session state accessors, persistence helpers, transcript rewind restore,
  permission-risk side queries, and lightweight runtime state snapshots now
  live in the dedicated `session_runtime::state` child module, keeping these
  compatibility helpers separate from engine construction.
- Late-bound agent, skill, fork-dispatch, hook-runner, task, task-list, and
  agent-transcript handles now live in the dedicated
  `session_runtime::handles` child module, including the scoped child-engine
  helper used by hook-agent execution and the team-context snapshot helper used
  by per-turn reminder wiring.
- Model-role overrides, thinking-effort rebinding, and the live `/status`
  report now live in the dedicated `session_runtime::roles` child module,
  keeping model-picker/session-status compatibility code out of the main
  runtime body.
- Live permission overlay preparation and permission-update persistence now
  live in the dedicated `session_runtime::permissions` child module, along
  with the live-permission base constructor used by session bootstrap and
  headless/SDK entrypoints. Engine config mutation, cache-break reset, todo
  snapshots, generic runtime accessors, the file-history transcript sink, the
  file-history checkpointing gate, and the orchestration context factory have
  moved under `session_runtime::state`.
- Model-role override state, thinking-level construction, configured-model
  lookup, and SDK/user model selection parsing now live in
  `session_runtime::roles`, with the existing `RoleOverride` and test helper
  paths re-exported from the parent module.
- Per-turn `QueryEngine` construction, SDK/headless config-based engine
  construction, fork-engine construction, context analysis, and the shared
  engine wiring pass now live in the dedicated `session_runtime::engine`
  child module. The main runtime body now keeps the build-time container
  assembly separate from per-turn engine assembly.
- Sandbox bootstrap and the shared settings-deny-path helper now live in the
  dedicated `session_runtime::sandbox` child module, with the old
  `crate::session_runtime::{build_sandbox_state,sandbox_settings_deny_paths}`
  paths re-exported for existing bootstrap and hot-reload callers.
- Session runtime startup assembly now lives in the dedicated
  `session_runtime::build` child module, keeping the parent
  `session_runtime.rs` file focused on the runtime option/state type
  definitions and shared intra-module helpers.
- A local `SessionHandle` wrapper now exists around `Arc<SessionRuntime>`;
  TUI, SDK, and headless startup paths construct sessions through this
  handle, and `QueryEngineRunner` holds the handle instead of directly owning
  the runtime `Arc`. Startup-owned background session consumers
  (cron tick, leader inbox poller, skill watcher, and post-login OpenAI model
  refresh) now accept `SessionHandle` at their public spawn/install
  boundary. Shared post-build late-bind installation now also enters through
  `SessionHandle`, and the agent-team wiring factory, fork dispatcher, and
  hook-agent runner installers sit behind that boundary. The agent-team wiring
  factory and the fork/hook implementations now also retain `SessionHandle`
  internally instead of capturing raw runtime `Arc`s. Unified MCP bootstrap now
  also takes `SessionHandle` instead of a raw runtime `Arc`, so the shared TUI /
  SDK / headless startup helpers are moving behind the same boundary. The SDK
  server state now stores the process session as `SessionHandle` and installs
  it through `with_session_handle`, leaving raw runtime access as an explicit
  compatibility escape hatch. SDK hook callback installation and initialize hook
  registration now also accept `SessionHandle` directly. Shared session-rename
  helpers now also take `SessionHandle`, so TUI / SDK rename paths and post-plan
  auto-title writes no longer require a raw runtime `Arc`. Live permission-mode
  runtime updates now also enter through `SessionHandle`, keeping TUI permission
  toggles on the same boundary. The TUI agent driver, slash dispatcher, slash
  queue drain, provider-login refresh path, slash-spawned submit turns, and
  auto-title task launch now carry `SessionHandle` through their local
  boundaries instead of rebuilding handles from raw runtime `Arc`s. TUI command
  queue history turns and prompt-mode bash response turns now also carry
  `SessionHandle` through the spawned task boundary. TUI auto-memory drain,
  manual `/dream` and `/summary`, and `/add-dir` helpers now also sit behind the
  same handle boundary. TUI resume hydration, `/resume` target resolution, and
  current-session plan-file path helpers now take `SessionHandle` as well. TUI
  fork-skill, manual compact, clear, `/btw`, export, tag, and provider
  status/logout helpers have also moved to the handle boundary. TUI goal
  command helpers, including status modal construction, goal-status transcript
  append, and active-goal snapshot persistence, now take `SessionHandle` too.
  TUI reload, permissions mutation, color, and context-inspection helpers now
  also accept `SessionHandle`. TUI auto-truncate, explicit rewind, and
  summarize-rewind helpers now use `SessionHandle` as their runtime boundary.
  TUI skill-override writes, plugin dialog payloads, permissions-editor payloads
  and persisted edits, agent create/open/delete refresh flows, and model role /
  thinking-effort updates have also moved to `SessionHandle`. Production
  `app/cli` call sites no longer expose raw `Arc<SessionRuntime>` in TUI helper
  signatures; raw runtime `Arc` ownership remains contained in `SessionHandle`
  itself, test fixtures, and documented compatibility escape hatches. SDK
  goal-status persistence helpers now likewise accept `SessionHandle` instead
  of raw runtime access. TUI-local plan-file and rewind helpers now keep
  `SessionId` typed through their signatures, converting to `&str` only at
  legacy context/file-history storage calls. Headless local goal transcript
  persistence and the runtime tool-result replacement seed now also accept
  typed `SessionId`, with SDK/TUI/headless resume paths converting only at the
  transcript storage boundary. Coordinator-mode persistence now also receives
  typed `SessionId` from SDK/headless exit paths before converting at
  `SessionManager::save_mode`.
- Public retarget entrypoints are now `retarget_for_new_session` and
  `retarget_for_loaded_session`; `/clear` reuses the new-session entrypoint
  after its pre-clear cleanup, while resume paths use the loaded-session
  entrypoint.
- `clear_conversation_rotates_session_and_preserves_permission_grants`
  covers the current `/clear` compatibility behavior: session id rotates,
  conversation-local state resets, and live permission grants survive.
- Session-id read paths now snapshot a typed `SessionId` through
  `QueryEngineConfig.session_id`; file watcher registration also snapshots
  from that config instead of carrying a separate mutable session-id handle.
- File-history snapshot persistence now derives its session id from the
  synchronized engine config mirror, so it no longer participates in the
  legacy retarget seam.
- The legacy retarget seam now accepts a typed `SessionId`: callers convert at
  the SDK/TUI compatibility boundary, `/clear` uses `SessionId::generate()`,
  and engine config / usage accounting / memory / model-runtime retargeting
  all derive from the typed id instead of passing raw strings through the
  mutation path.
- Memory runtime/session-memory/extract/dream services now store their active
  session id as `SessionId`; `/clear` retargeting passes the typed id through
  the runtime and converts to string only for session-memory paths and other
  on-disk transcript/storage boundaries. The composed `MemoryRuntime` now
  shares one session-id slot across extract, dream, and session-memory
  services, so memory retargeting performs one typed slot update instead of
  three independent service writes. `SessionRuntime` no longer stores a
  duplicate `session_memory_service` field; per-turn engine wiring derives the
  service handle from `memory_runtime.session_memory`.
- The misleading `SessionRuntime::start_new_session` entrypoint is gone;
  SDK `session/start` now calls an explicitly empty-session retarget seam,
  while SDK `session/resume` and TUI `/resume` call the explicitly loaded-
  session retarget seam.
- Loaded-session resume and fresh-session retargets now enter the unified
  `retarget_runtime_for_session` seam, with the lower
  `retarget_session_id_boundaries` mutation kept as the identity-only shim.
- `retarget_session_id_boundaries` now delegates to named sub-boundaries for
  engine config, memory session id, and model-runtime session header
  variables, so each remaining in-place mutation has a distinct demolition
  point. The model-runtime header-var refresh now accepts typed `SessionId`
  instead of reopening a raw string boundary at the inference layer; the
  header-template snapshot stores `${SESSION_ID}` as `Option<SessionId>` and
  converts to `&str` only during template expansion.
- SDK `session/start`, SDK/TUI loaded-session resume, and TUI `/clear` now
  converge on one `SessionRuntime::retarget_runtime_for_session` compatibility
  seam with an explicit empty-vs-loaded usage-ledger mode; session-keyed
  runtime caches, transcript dedup, tool-result replacement state, denial
  tracking, and cache-break detectors reset from that single demolition point.
- The fused-runtime retarget seam now lives in the dedicated
  `session_runtime::retarget` child module instead of the main
  `session_runtime.rs` body, leaving public callers on `retarget_for_new_session`
  / `retarget_for_loaded_session` while isolating the eventual runtime-registry
  demolition point.
- File-history sink session identity now comes from the synchronized
  engine-config mirror; normal runtime identity reads no longer have a
  separate mutable reader or wrapper module.
- The detached hook orchestration factory now reads `SessionId` from the
  synchronized `QueryEngineConfig` snapshot instead of maintaining a separate
  session-id mirror. File-history snapshot persistence also reads that mirror
  and converts only at its existing legacy output boundary.
- `SessionRuntime::build` now validates its initial session id with the
  checked `SessionId` constructor before seeding live identity and engine
  config state.
- `UsageAccounting` construction and retarget APIs now require a typed
  `SessionId`, so the accounting layer no longer performs unchecked
  conversion from raw strings.
- Query tool-permission preparation now carries `SessionId` through
  `ToolCallRunner`, `PendingToolPreparation`, and `PermissionController`,
  converting only when filling the legacy permission bridge request field.
- SDK query result DTOs now store the root `session_id` as typed
  `SessionId` while preserving the same string-shaped serde output.
- `AgentRunIdentity.session_id` is now a typed `SessionId`, so subagent/fork
  query configs carry a checked parent-session identity before they reach
  `QueryEngineConfig` or child usage accounting.
- `AgentSpawnRequest.session_id` is now `Option<SessionId>` instead of a
  string/empty-sentinel field; serde rejects unsafe ids, and coordinator
  subagent / teammate spawn paths enter through
  `AgentSpawnRequest::parent_session_id()` before constructing child identity,
  metadata paths, pane parent-session config, or in-process teammate runner
  config.
- Background-agent resume now receives the parent session id as `SessionId`
  through the `AgentHandle::resume_agent` trait and coordinator implementation
  before reading transcript metadata or rebuilding the resumed
  `AgentSpawnRequest`; transcript store calls remain explicit string
  boundaries.
- Implicit session-team bootstrap now reads typed runtime identity and derives
  the deterministic `session-<id[:8]>` team name from `SessionId`; the
  `InitializeSessionTeamRequest.leader_session_id` field is typed as
  `SessionId`, with serde rejecting unsafe ids before roster/team state is
  written or reused; team-file persistence remains an explicit string
  boundary.
- Coordinator roster commit/build requests and persisted `TeamMember` /
  `TeamFile` Rust fields now carry teammate and leader session identity as
  `SessionId` internally while serde keeps the on-disk JSON fields
  string-shaped.
- `hub/protocol` has moved to `coco-event-hub.v2`: `EventEnvelope` now carries
  typed `SessionId`, optional typed `AgentId`, and per-session
  `session_seq: i64`; `AnnounceAckFrame.resume_from` and
  `BatchAckFrame.up_to_seq` are
  `HashMap<SessionId, i64>` cursor maps. `hub/server` advertises the v2
  subprotocol/schema and its read-model `EventRow` exposes `session_seq`
  instead of a global `seq`; hub server row/query structs now keep session
  identity as `SessionId` and row turn identity as `TurnId` internally while
  URL/query params remain explicit string parse boundaries. `AnnounceFrame`
  carries the instance's live-session set as `Vec<SessionId>`, so hub
  `resume_from` maps are scoped instead of unbounded. `event-hub/spec.md` §4/§5
  now describes the
  multi-session identity and cursor model instead of the old single-session v1
  frame shape.
- `coco-bridge` IDE/REPL protocol DTOs and bridge-local session state now carry
  `SessionId` for status/result messages and activity cache keys while keeping
  the serialized `session_id` field string-shaped for IDE/SDK consumers.
- `hub/connector` now has the typed egress conversion boundary from
  AppServer-stamped `SessionEnvelope` to Hub v2 `EventEnvelope`: durable
  Protocol envelopes become `{ instance_id, session_id, agent_id?, session_seq,
  ts, schema_version, payload }`, ephemeral envelopes are skipped, and a
  sequenced non-Protocol envelope is rejected as a stamping taxonomy violation.
  The connector also has a batch helper that preserves durable envelope order
  while filtering live-only envelopes before constructing `BatchFrame`.
- Hook orchestration now carries checked session identity through
  `coco_hooks::OrchestrationContext.session_id: SessionId`; hook JSON/env
  conversion stays at the legacy `BaseHookInput` / command execution boundary,
  and coordinator subagent, worktree, and teammate-idle hook paths no longer
  use the previous empty hook session id fallback.
- `ToolUseContext.session_id_for_history` remains optional/string-shaped for
  embedding compatibility, but Agent, Workflow, Skill, and stopped-agent
  SendMessage resume paths now read it through a checked `SessionId` helper
  before constructing child-spawn or resume routing state.
- `SubagentInheritance.session_id` is now a typed `SessionId`, so fork-mode
  skill subagents carry checked parent-session identity from `SkillTool` /
  `SessionRuntime::invoke_skill_fork` into `QuerySkillRuntime` without a
  string round-trip.
- `WorkflowSpawnContext.session_id` is now `Option<SessionId>`, so workflow
  `agent()` calls retain checked parent-session identity inside the local
  workflow host and pass it through the typed `AgentSpawnRequest` boundary.
- Fork dispatch now snapshots the parent session as `SessionId` and threads it
  through `forked_agent::build_query_config` / `QueryEngineConfig` without a
  string round-trip; sidechain transcript persistence converts only at the
  existing agent-transcript store boundary.
- Headless print-mode bootstrap now validates the CLI/override session id as
  `SessionId`, threads typed identity into `QueryEngineConfig`, and converts to
  string only for header vars, local transcript persistence, replacement-state
  reads, and coordinator-mode resume storage.
- SessionRuntime local metadata/transcript writes, task-output bootstrap, and
  session rename now snapshot the runtime identity as `SessionId` first, then
  convert only at the existing JSONL/path/session-manager string boundaries.
- TUI prompt-history hydration/persistence, branch transcript paths, session
  plan paths, compact/exit metadata re-append, tag toggles, and auto-title
  checks now snapshot the current runtime identity as `SessionId` before
  converting at legacy history/path/session-manager boundaries.
- TUI submit and queued slash-engine turns now carry `SessionId` through the
  runner task boundary into `process_submit_turn`; file-history, compact
  metadata, and auto-title state convert only at their existing string-keyed
  APIs.
- `tui_runner.rs` no longer calls the legacy string session-id getter; UI hints
  and protocol notifications derive their string payloads from typed runtime
  snapshots at the boundary.
- `SessionRuntime::current_session_id()` has been removed; runtime callers now
  use `current_typed_session_id()` and convert explicitly only at legacy string
  API boundaries.
- `SessionIdentitySnapshot` no longer exposes an implicit string getter; skill
  command registry late-binding and identity tests now go through typed
  snapshots before converting at the string boundary.
- The extra `SessionIdentitySnapshot` wrapper and `SessionIdentityReader` are
  gone; runtime session-id reads now return `SessionId` from the engine config,
  and command-registry late binding now stores a typed `SessionId`, converting
  only at the skill prompt `${CLAUDE_SESSION_ID}` substitution boundary.
- `SessionRuntimeBuildOpts.session_id_override` is now `Option<SessionId>`;
  runtime construction receives checked identity directly and mints fresh
  sessions with `SessionId::generate()` instead of validating raw strings.
- Headless `RunChatOptions.session_id_override` is now `Option<SessionId>`;
  resume/fork plan ids and `--session-id` are validated at the print-mode edge
  before runtime construction.
- `resume_resolver` now mints fork destination ids with `SessionId::generate()`
  and validates explicit `--session-id` values before copying fork transcripts.
- `ResumePlan.session_id` and `ResumePlan.source_session_id` are now typed
  `SessionId`s; TUI resume/branch hydration and headless handoff convert only
  at transcript/protocol/task-panel string boundaries.
- `QuerySkillRuntime` now stores its prompt-substitution session id as
  `SessionId`, and plan/file-history tool paths (`ExitPlanMode`,
  `VerifyPlanExecution`, TodoWrite snapshot fallback, and edit history
  tracking) enter through `ToolUseContext::checked_session_id_for_history()`
  before converting to legacy string persistence APIs.
- Plan-mode reminder side-effect state and history reset helpers now accept
  typed session ids, converting to protocol strings only when emitting
  `SessionResetForResume`.
- In-process teammate runner configs now carry the parent session as
  `SessionId`; `TeammateExecutionAdapter` and periodic AgentSummary forks
  construct `AgentRunIdentity` from typed identity instead of re-validating
  raw strings.
- Teammate task-local/dynamic identity contexts now store parent session
  identity as `SessionId`; the env-derived fallback is exposed through a
  checked accessor while the legacy string getter remains an explicit boundary.
- Pane teammate spawn config now carries parent session identity as
  `SessionId` and converts to `COCO_PARENT_SESSION_ID` only at the inherited
  environment boundary.
- `QueryEngineConfig::default()` and remaining local test fixtures now use
  checked `SessionId` constructors instead of unchecked empty/string
  constructors.
- `SessionRuntime` exposes a typed current-session snapshot for internal
  wiring, and skill-runtime installation now uses it instead of the legacy
  string session-id getter.
- `SessionId` / `AgentId` now expose compatible checked constructors for
  path-safe ids, and `SessionId::try_new_uuid` covers the canonical
  server-generated UUID shape. New SDK `session/start` ids now come from
  `SessionId::generate()` instead of the legacy `session-<uuid>` prefix.
- `AgentId::try_new_generated` / `try_generate` now validate the canonical
  generated shape (`a[optional-label-][16-hex-chars]`) while leaving the
  compatibility `try_new` path available for historical agent ids; BgAgent
  task id generation and framework fork auto ids now reuse the same canonical
  generator.
- `QueryEngineConfig.agent_id` and `ForkContextOverrides.agent_id` now carry
  `AgentId` internally; query prompt/compaction/reminder/history paths convert
  only at existing transcript, plan-file, command-queue, and protocol string
  boundaries.
- `AgentRunIdentity.agent_id` now carries `AgentId` internally, so child query
  engines receive a checked child identity; usage attribution, structured
  workflow labels, teammate task routing, and transcript persistence convert
  only at existing string-keyed boundaries.
- `SessionId` / `AgentId` inner fields are now private, and serde
  deserialization routes through the checked path-component validation
  instead of accepting arbitrary strings.
- `SessionId` / `AgentId` no longer expose unchecked `new` or raw
  `From<String>` / `From<&str>` constructors; call sites now use checked
  constructors, canonical generators, or the documented `TaskId`→`AgentId`
  reinterpretation for BgAgent task routing.
- SDK server `SessionHandle` / `TurnHandoff` now carry `SessionId` internally;
  `session/start`, `session/resume`, per-turn stats forwarding, QueryEngine
  handoff, rewind file-history calls, and SDK test runners convert to strings
  only at protocol or legacy persistence boundaries.
- Hook and analytics DTOs now type their session identity at the Rust boundary:
  `BaseHookInput.session_id`, `AnalyticsEvent.session_id`, and
  `AnalyticsLogger.session_id` use `SessionId` while hook JSON/env and
  telemetry serialization remain string-shaped. CLI `OutputEvent::SessionMeta`
  likewise stores `SessionId` internally.
- Remaining raw `session_id: String` fields are now concentrated in explicit
  external/user-input seams: CLI subcommand args, app-query mock harness
  params, exec-server/bridge/hub/retrieval protocols, and the Google
  CodeAssist onboarding API DTO.
- The fused-runtime retarget seam now distinguishes empty `session/start`
  retargets from loaded `session/resume` / TUI `/resume` retargets; usage
  accounting resets for new SDK sessions and loads persisted snapshots only
  on resume-style paths.
- Remaining identity work: remove in-place session id rotation entirely
  during the runtime split.

**Phase B — atomic cut-over (single PR):**

Route TUI and headless through `LocalClientAdapter`; route SDK through
`JsonRpcAdapter`; delete the old stack. Demolition list:

- `retarget_for_loaded_session`, `retarget_session_id_boundaries`, and the
  `/clear` id-rotation path.
- The fused `SessionRuntime` struct itself, including its in-place
  engine-config / memory / model-runtime session-id retarget seam.
- `SdkServerState.session` singleton slot
  (`sdk_server/handlers/mod.rs:236`) and the dispatcher's
  archive→start session-cycling (`dispatcher.rs:194-206`).
- `hub/protocol` v1 frames (`AnnounceAckFrame.resume_from`,
  scalar `BatchAckFrame.up_to_seq`, global-`seq` `EventEnvelope`).
- `event-hub/spec.md` §4 single-session identity prose.
- The process-boot single fold of project/local settings into one
  process-wide `RuntimeConfig` (replaced by the per-session fold, §6.5).
- `std::env::current_dir()` fallbacks on session-scoped paths
  (`engine_session.rs:565`, `tool_call_preparer.rs:600`,
  `engine_prompt.rs:98`) and `absolutize`'s process-cwd default for
  session paths.

**Phase C — surfaces (multi-attach):** passive surfaces, multiple
surfaces per connection, interactive-conflict rejection, replace/archive
surface routing (§11.5).

**Phase D — external adapters:** `WebAdapter`, `DesktopAdapter`,
`ImGatewayAdapter` as separate crates (§12.3–12.5), each keeping platform
credentials and capability state outside AppServer core.

Phase A/B ordering note: the runtime split (A3) lands *before* the
cut-over so AppServer never wraps the fused `SessionRuntime` — routing
ownership and state ownership move together.

## 19. Testing Plan

Registry and lifecycle:

- Concurrent resume of one non-live session performs exactly one load
  (single-flight); load failure removes the entry; the next resume
  retries.
- Close-on-`Loading` awaits the load then closes cleanly.
- Replace: Stage-1 failure leaves old fully intact (MCP alive, queue
  undrained); commit re-points the calling surface atomically; `Closing`
  serves `get`/`resume` until cascade completion; max_sessions +1
  accounting under a racing `session/start`.
- Close cascade order is observable: queued turns get `turn/interrupted`
  before MCP teardown; `task_set` drains before entry removal; JSONL
  survives and is re-openable.
- Closing sessions reject new turn starts with a typed error; `resume`
  during `Closing` awaits the cascade then reopens from disk with a
  fresh config fold on the same transcript.
- `create` failure (e.g. PerSession MCP spawn error) removes the
  `Loading` slot and frees its `max_sessions` slot.
- The driver mailbox stays live during a long turn: `Interrupt`,
  `ReadState`, and `Steer` are served while `TurnActive` (no inline-turn
  starvation).
- An abandoned load — every awaiting caller cancelled mid-`Loading` —
  still completes: the spawned owner task promotes or removes the slot
  on its own (§7.2).
- A wedged active turn: close proceeds after `turn_drain_timeout_secs`
  and the slot leaves `Closing` (§7.4 step 3).
- Process shutdown drains all live sessions concurrently within
  `shutdown_timeout_secs`; a timeout forces abort with transcripts
  already flushed; the exit code distinguishes drain from abort (§7.7).

Concurrency and events:

- Two sessions run turns concurrently with no event mixing; aggregation
  keyed `(session_id, turn_id)` never merges independent sessions.
- Envelope completeness: every session-scoped envelope has `session_id`;
  every turn-scoped envelope has `turn_id` (assert at the stamping seam).
- `session_seq` is strictly monotonic per session and shared with hub v2
  acks.
- `session_seq` survives a process restart: archive, restart, resume →
  new durable events continue above the persisted watermark; a hub
  cursor taken before the restart replays without overlap or regression.
- Crash restart (kill without flush) then resume: the skip-ahead
  counter never re-issues a seq at or below any previously emitted one;
  the hub rejects a regression if one is ever presented (§10.3).
- Ephemeral events (`Stream` deltas, `Tui`) carry `session_seq: None`
  and never enter the ring; the durable stream has no seq holes within
  a process epoch; a mid-turn reconnect re-baselines from snapshot +
  next boundary event without replayed deltas.
- Mailbox FIFO contract; `McpOauth` refresh proceeds during a long
  turn; `ProcessConfig` writes hold no session lock; concurrent
  `archive`+`replace` on one id: exactly one wins, the other gets the
  typed slot-state error.
- Slow consumer: filling one connection's outbound queue disconnects only
  that connection; the emitter and sibling surfaces are unaffected.
- Ring replay: `after_seq` inside the ring replays exactly the gap;
  outside the ring returns `snapshot_required` and `session/read`
  re-baselines.
- Subscribing during live emission: the paired lock sections (§10.2)
  yield no gap and no duplicate at the replay→live boundary.

Projects and cwd:

- Two sessions in different project roots resolve different
  project/local settings: permission rules and hooks from project A
  never apply to a session in project B.
- Two sessions with different cwds run shell and file tools concurrently
  with no path bleed (spawn `current_dir`, relative resolution, ignore
  rules).
- Same project root → one shared `ProjectServices` instance (assert by
  identity); different roots → independent instances; idle eviction
  reloads changed settings for the next session.
- Resume re-folds config against the recorded cwd; a project settings
  change made while the session was archived is visible after resume.
- A project-defined MCP server is visible only to sessions of its
  project; on a name collision with a user-level server, the session's
  effective set resolves to the project definition.
- A project-defined `Shared` MCP instance is shared within its project
  and isolated across projects (distinct processes, distinct spawn cwd);
  `ProjectServices` eviction tears it down; credentials for same-named
  servers in different projects never mix.
- Lint seam: session-owned production code rejects new
  `std::env::current_dir` reads via `check-session-cwd-discipline.sh`;
  the CLI startup boundary and documented headless embedder fallback stay
  on the allow-list. Full-workspace `clippy.toml` enforcement is the
  steady-state follow-up once standalone tools are split or allow-listed.

Surfaces and clients:

- One session, multiple passive surfaces; passive reconnect never
  acquires interactive ownership.
- Second interactive attach → `InteractiveOwnerConflict` with owner
  metadata.
- One connection hosts surfaces on multiple sessions; replace re-points
  only the calling surface; peers get `session/replaced` and are
  detached, not migrated.
- Surface-declared capability gates server-initiated requests; undeclared
  → request never sent.
- SDK dual-channel disconnect: in-flight RPCs resolve `Disconnected` AND
  event streams terminate; subsequent calls `ClientInvalid`; documented
  resume flow recovers.
- `replace`/`close` failure returns the original handle
  (`Err((self, e))`) and the session remains usable through it.
- `PassiveSessionClient` cannot start or interrupt turns (compile-time:
  no such methods); attaching it never conflicts with the interactive
  owner.
- Orphan policy: dropping every connection leaves the session `Live` and
  listed with surface count 0; with `idle_session_timeout_secs` set, an
  idle surfaceless session archives; an orphan with a running background
  turn does not.
- TUI via `LocalClientAdapter` behaves exactly like today's single-session
  app (snapshot tests unchanged).

Adapters:

- IM `/stop` interrupts only the mapped session's turn; platform tokens
  never appear in AppServer core (grep-level test, Hermes-style).
- Hub v2: per-session `resume_from` replays each session independently;
  no `thread_id` anywhere.

## 20. Non-Goals (v1)

- Multi-session TUI UI.
- Public TCP/WebSocket listeners by default.
- Changing the JSONL storage layout; any event-sourced store.
- `ThreadId` or any second root identity.
- Web/desktop/IM adapters reading JSONL directly.
- Platform credentials, IM tokens, file-picker handles, keychain state,
  or callback tokens inside AppServer core.
- Cross-session filesystem isolation.
- Interactive takeover (v2), multiple interactive owners (not planned).
- Client-supplied per-session config overlays beyond the
  `SessionStartParams` knobs. (The per-session fold in §6.5 derives from
  settings *files* — process + project layers — not from request
  payloads; a session in project A legitimately resolves different
  config than one in project B.)
- Transcript deletion semantics (close ≠ delete).

## 21. Open Questions

- v1 `SurfaceCapabilities` field set for desktop (which of attestation /
  notifications / file picker / keychain are schema-stable now vs gated
  experimental per §8.2).
- Default sizing for the retention ring and surface limits (§17) — start
  with the listed defaults, revisit with real fan-out numbers.
- `ImGatewayAdapter` persistence format for the `channel key → SessionId`
  map (gateway-local file vs reusing `SessionCatalog` metadata).
- Local transport authentication: UDS relies on socket file
  mode/ownership in v1; the flag-gated WS listener needs a token scheme
  (reuse the bridge JWT machinery vs a new scheme) before it can
  default on.

## 22. Decision Log

Carried forward from v5 unless marked; new and reversed entries noted.

| # | Decision |
|---|---|
| D-1 | `session_id` mandatory on every session-scoped request; no implicit defaults |
| D-2 | Server-generated `SessionId` (UUID v4); clients cannot propose |
| D-3 | `session/resume` idempotent: rejoin running OR single-flight load from disk |
| D-4 | `/clear` discards all state; no carry-forward; implemented as `replace` |
| D-5 | `session/replace` is the atomic primitive; two-phase commit per §7.5; Stage 2 single write-lock section with zero `.await`; `max_sessions` +1 transient |
| D-6 | Close cascade order fixed (§7.4); `task_set.shutdown().await`, never strong-count polling |
| D-7 | `ResumeError` is `Clone` (Shared-future bound); IO errors converted to `(kind, message)` at the load site |
| D-8 | Registry slots: `Loading`/`Live`/`Closing`; all count toward `max_sessions`; one slot per id |
| D-9 | True single-flight load (placeholder future), not codex-style optimistic dedup |
| D-10 | Session state is actor-owned (`SessionRuntime` driver + `SessionCommand`); `SessionHandle` is a cheap clone with immutable `session_id`; no cross-session locks |
| D-11 | Engine/core crates never depend on server crates; shared types live in `coco-types` (codex counter-lesson) |
| D-12 | Canonical protocol types stay in `coco-types` v1; `app/server-protocol` crate deferred; codex-style macro + schema-gen adopted when Web/Desktop need it |
| D-13 | `ConnectionKey` private, never wire/disk; `SurfaceId` public wire id, never persisted |
| D-14 | Event tagging via `SessionEnvelope` stamped at one routing seam; per-variant `session_id` fields removed; `turn_id` typed `TurnId` everywhere |
| D-15 | Reconnect = snapshot + per-session `session_seq` replay over a bounded ring; no retained-cursor scheme beyond it |
| D-16 | Hub speaks the same `session_seq`; `coco-event-hub.v2` with per-session cursor maps; **v1 hub frames deleted (no compat)**; `event-hub/spec.md` §4 revised in the same change |
| D-17 | Slow consumer: bounded per-connection outbound queue (1024), disconnect on full, never block emitters |
| D-18 | ~~Three serialization queues~~ — superseded by D-41: session order = driver mailbox, lifecycle order = slot state machine; `McpOauth(server)` and `ProcessConfig` remain as real queues; subagents bypass the mailbox |
| D-19 | Process state is snapshot-at-session-start (`ArcSwap`/`watch`); reloads never mutate running sessions |
| D-20 | MCP `McpScope::{Shared,PerSession}`; PerSession ≠ per-session OAuth; pdeathsig + PID-file reaping. Refined by D-45 for multi-project |
| D-21 | Approval replies validated against `(session_id, prompt_id)`; mismatch → `WrongSession` |
| D-22 | `session/list` / `session/read` / `session/turns/list` three-tier pagination; passive surfaces use these for history (no separate snapshot API) |
| D-23 | Exactly one `Interactive` surface per session in v1; conflict → typed `InteractiveOwnerConflict` carrying owner metadata; takeover is v2 (jcode model) |
| D-24 | **Reverses v5 #6**: one connection hosts many surfaces across many sessions; SDK contract is `ServerClient` + per-session `SessionClient`; `replace(self)`/`close(self)` consume the handle |
| D-25 | Capabilities declared and enforced per surface, not per connection (fixes codex shared-thread capability scoping) |
| D-26 | Replace/archive notify peer surfaces and detach them; peers migrate themselves (no silent re-subscription) |
| D-27 | Dual-channel disconnect, synthesized client-side; recovery = reconnect + `resume_session(saved_id)`; `Drop` silent, `close()` explicit |
| D-28 | TUI `/quit` and `/resume` archive the current session explicitly (registry has no auto-GC) |
| D-29 | `max_sessions` default 32; `ResourceExhausted`, no eviction; all limits in §17 via `EnvKey` |
| D-30 | IM gateway owns the durable `channel key → SessionId` map (OpenClaw two-level identity); platform tokens/capabilities never enter core (Hermes boundary) |
| D-31 | No dual-stack migration: parallel build, single atomic cut-over, demolition list in §18 |
| D-32 | `cwd` restored from transcript metadata on resume; no `current_dir()` fallback; missing cwd → `ResumeError::CwdNotFound` unless `cwd_override` given |
| D-33 | Cross-session filesystem isolation documented as a non-goal |
| D-34 | Worktree governed solely by `Feature::Worktree` as resolved in the session's config fold; no separate `session/start` parameter |
| D-35 | Three ownership scopes: Process / Project / Session. `ProjectServices` cached per project root (git worktree root, else cwd) with single-flight load and idle eviction — the opencode `LocationServiceMap` shape. Evidence: all five reference products host multiple working directories per process; none binds one process to one project root |
| D-36 | **Amends v5 #17**: only policy/user/flag/env settings layers are process-global. Project + local layers fold per session at `session/start` against the session's cwd (codex layer-stack position: file layers below per-session `SessionFlags`-style overrides); `RuntimeConfig` and `Features` are per-session snapshots; resume re-folds |
| D-37 | cwd is per-session state threaded explicitly (`ToolUseContext` → spawn `current_dir`); no process-cwd fallbacks on session paths; currently enforced on session-owned production crates via `check-session-cwd-discipline.sh`, with full-workspace `clippy.toml` `disallowed-methods` as the steady-state target once standalone tools are split or allow-listed |
| D-38 | Event taxonomy: durable (Protocol + boundary events — sequenced, ring-retained, hub-shipped) vs ephemeral (Stream deltas + Tui — `session_seq: None`, live-only, never replayed); decided at the stamping seam |
| D-39 | `session_seq` survives restarts: high-water mark persisted in transcript metadata (periodic flush + close cascade step 5), counter restored on resume; hub cursors stay valid across process restarts |
| D-40 | The driver never runs a turn inline: turns are spawned tasks; every mailbox command is fast; `Interrupt`/`ReadState`/`Steer` are served during `TurnActive`. The session root `CancellationToken` is private to runtime + registry close path — not on the handle |
| D-41 | Serialization: session order = driver mailbox; lifecycle order = slot state machine (archive/replace are registry ops, never mailbox commands); only `McpOauth` and `ProcessConfig` are separate queues (amends the v5 three-queue framing) |
| D-42 | `resume` during `Closing` reopens (await close future → single-flight disk load), never hands out a draining runtime's handle; `create` occupies a `Loading` slot with full `max_sessions` accounting, same shape as resume |
| D-43 | Two-lock taxonomy: registry lock → single `RoutingState` lock (connections/surfaces/forward/reverse/pending requests under ONE lock), fixed order, no `.await` under either; replace Stage 2 is the only path taking both |
| D-44 | Orphan policy: sessions end only via archive/replace/process shutdown or opt-in `server.idle_session_timeout_secs`; `session/list` exposes surface counts. SDK: `replace(self)`/`close(self)` return `Err((self, error))` — a live session's handle is never silently lost; passive observation is the method-restricted `PassiveSessionClient` type |
| D-45 | MCP definitions fold like config: user catalog (process) ∪ project `.mcp.json` (ProjectServices), project wins on name collision, project servers gated by per-project approval. `Shared` shares at the defining layer's scope — project-defined `Shared` instances are keyed `(project_root, server_name)`, spawn with cwd = project root, and die with the `ProjectServices` entry; credentials and the `McpOauth` queue are keyed by definition site |
| D-46 | Single-flight load, close cascade, and the whole replace operation run in spawned owner tasks that perform the slot transitions themselves; `SharedLoadFuture`/`SharedCloseFuture` are completion signals only — slot progress never depends on callers staying alive (a caller-driven future stalls unpolled when every awaiter is cancelled; amends D-9 mechanics, extends D-6) |
| D-47 | seq crash recovery: resume initializes the counter to `watermark + event_retention_per_session` (skip-ahead); "no seq holes" holds per process epoch; replay is `seq > cursor` everywhere, never contiguity-based; the hub rejects per-session seq regressions (amends D-39) |
| D-48 | `ProjectServices` holds project-only contributions — never pre-merged with process sources; the §6.5 fold merges at session start. Entry splits into a stat-fingerprinted `ProjectConfigSnapshot` (re-read in place when stale, making resume-sees-current-settings true without eviction) and lifetime-bound `ProjectHeavyServices` (LSP/retrieval/project-`Shared` MCP survive config re-reads, die on eviction) (amends D-35/D-36 internals) |
| D-49 | Close cascade is registry-initiated, driver-executed, supervisor-completed: registry cancels the token and sends `Close`; the driver runs the drain steps over its own state; a spawned supervisor awaits the driver `JoinHandle`, removes the slot, completes the close future. `SessionCommand::Close` is lifecycle mechanism, never a client command (sharpens D-41); cascade step 3 is bounded by `server.turn_drain_timeout_secs` |
| D-50 | Registry and `RoutingState` locks are `std::sync::RwLock`, not tokio locks (the no-await-under-lock rule makes async locks pure overhead; §7.3's sync accessors follow). Registry/server error enums use snafu per the Tier-3 one-library rule — `thiserror` stays in the Tier-2 transport/client crates (amends D-43 lock choice, D-7 derive) |
| D-51 | Ring append + fan-out delivery is one `RoutingState` lock section per envelope; subscribe replay-read + registration is one section — the pairing guarantees no gap/duplicate at the replay→live boundary. The driver mailbox is bounded (64) with awaited sends and no drop path, so `Interrupt` cannot be lost |
| D-52 | JSON-RPC frame types live in `coco-app-server-transport` (wire format, not domain); `coco-types` keeps only canonical requests/notifications/envelope (refines D-12) |
| D-53 | Fixed process-shutdown sequence (§7.7): stop transports → concurrent close cascades under `server.shutdown_timeout_secs` → hub egress flush → exit code reflects drain vs forced abort; second signal aborts immediately |
| D-54 | Wire counters (`session_seq`, `after_seq`, hub cursors) are `i64` per the workspace integer convention; the hub announce carries the instance's live-session set and ack cursor maps are scoped to it; `ModelRuntimeRegistry` keys cached provider clients by `ProviderClientFingerprint` (per-session folds may diverge per project) |
