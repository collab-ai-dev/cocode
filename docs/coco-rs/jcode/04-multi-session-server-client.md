# Multi-Session Server / Client Architecture: jcode vs coco-rs

> Source-verified comparison. Every claim below was checked against actual
> source on both sides; file:line refs are load-bearing. jcode paths are
> rooted at `/lyz/codespace/3rd/jcode`; coco-rs paths at
> `/lyz/codespace/codex/coco-rs` (or `/lyz/codespace/codex/docs/coco-rs` for
> plans). README marketing numbers are flagged where they back an
> architectural claim.

This module is where the two harnesses diverge most sharply, and where the
divergence is mostly a **maturity gap, not an architecture refusal**. jcode
ships a hardened single-process / multi-client peer server today; coco-rs
has the same single-process-shared-singletons *design* written down in a
1053-line plan, plus three narrower production pieces (a read-only
observability hub, a single-session SDK server, and a cloud-session client),
but **no multi-client server crate exists on disk yet**.

---

## jcode approach

jcode runs **one persistent server process** that owns all sessions; thin TUI
clients attach over a Unix domain socket and reconnect transparently across
disconnects and even across server self-reloads. This is the headline
`jcode serve` / `jcode connect` capability, and it is fully built — the
`src/server/` tree is ~60 files.

### Transport and framing

Newline-delimited JSON over `UnixListener` / `UnixStream`
(`src/transport/unix.rs:1-4`). Two sockets are published as a pair: the main
`jcode.sock` and a sibling `jcode-debug.sock`, derived by string-swap
(`src/server/socket.rs:16-38`) and cleaned up together
(`cleanup_socket_pair`, `socket.rs:41-46`). `Stream::pair()`
(`transport/unix.rs:20-22`) provides an in-process bridge for tests, and a
full Windows named-pipe implementation exists (`src/transport/windows.rs`).
**There is no TCP or WebSocket *server* transport** — jcode is reachable only
over a local Unix socket (or named pipe on Windows); remote attach needs an
external bridge (`ssh_remote.rs` / `sidecar.rs`).

### Daemon startup handshake (the strongest startup primitive)

The README's "first run spawns daemon, subsequent connects" is real and
unusually hardened. `spawn_server_notify` (`socket.rs:163-251`):

1. Creates an anonymous `pipe()` (`socket.rs:171-176`).
2. Sets `FD_CLOEXEC` on the read end the parent keeps, and **clears**
   `FD_CLOEXEC` on the write end inside `pre_exec` so it survives `exec`
   (`socket.rs:178-196`) — this CLOEXEC discipline is what stops the parent
   hanging to the 10 s timeout.
3. Calls `setsid()` in `pre_exec` (`socket.rs:194`) to fully detach the
   daemon.
4. Passes the write-end fd to the child via `JCODE_READY_FD`
   (`socket.rs:198`), then blocks on a 1-byte async read with a 10 s timeout
   (`socket.rs:209-238`), falling back to socket-poll on timeout / EOF / error
   (`wait_for_server_ready`, `socket.rs:225/232/236/254`).

The child calls `signal_ready_fd()` (`socket.rs:361-379`) — writing one byte
`b"R"` and closing the fd — **only after its accept loops are live**. The
parent therefore connects *exactly* when the daemon can serve, never
poll-and-pray. A single daemon is guaranteed by `flock(LOCK_EX|LOCK_NB)` on
`jcode-daemon.lock` (`try_acquire_daemon_lock`, `socket.rs:104-128`), and
spawn races are reconciled by parsing the loser's stderr
(`server_start_matches_existing_server` matches "Another jcode server process
is already running", `socket.rs:306-310`; `handle_server_start_exit` then waits
for the winning daemon, `socket.rs:342-359`).

### In-place `exec()` hot-reload preserving the socket

The README's `/reload` "server execs into new binary, same PID, clients
reconnect" is real. On a reload signal, `await_reload_signal`
(`src/server/reload.rs:50-192`) persists per-session reload-recovery intents,
gracefully shuts down sessions (`RELOAD_GRACEFUL_SHUTDOWN_TIMEOUT = 2s`,
`reload.rs:14`), unlinks the old socket pair, removes `JCODE_READY_FD`, detaches
stdio (`prepare_server_exec`, `reload.rs:16-30`), then `replace_process()`
execs `jcode serve --socket <same path>` (`reload.rs:160-163`). The socket path
is preserved, so reconnecting clients land on the new binary at the same PID.
`ServerEvent::Reloading { new_socket }` and `ReloadProgress` stream the rebuild
to the initiating client (`crates/jcode-protocol/src/wire.rs:950,958`). This is
the daemon-side of jcode's self-dev story (agent edits/builds/reloads its own
binary).

### Idle-timeout + owner-pid liveness

A configurable per-server lifecycle monitor (`spawn_temporary_lifecycle_monitor`,
`src/server/lifecycle.rs:138-192`) runs a 10 s tick loop: it tracks
`client_count == 0` via `Arc<RwLock<usize>>`, sets `idle_since` on first zero,
and exits after `policy.idle_timeout_secs` (default `DEFAULT_TEMP_IDLE_SECS =
30*60`, `lifecycle.rs:11`). It **also** self-terminates if its `owner_pid` is
gone (`process_alive` = `kill(pid,0)` true on `0`/`EPERM`,
`lifecycle.rs:152-161,217-232`). On shutdown it unregisters from the registry,
removes both sockets, cleans metadata, and exits a dedicated code
(`shutdown_temporary_server`, `lifecycle.rs:194-204`). Server scope/ownership is
also formalized on disk: a `<socket>.server.json` carrying
`schema_version/scope/pid/ppid/owner_pid/idle_timeout/argv`
(`TemporaryServerMetadata`, `lifecycle.rs:20-132`) for introspection and orphan
reconciliation.

### Runtime / dispatch shape (the central tech-debt)

`ServerRuntime` clones ~25 `Arc`-shared state fields and spawns three accept
loops (main, debug, gateway). Each accepted stream is `tokio::spawn`-ed into
`handle_client(...)` with a **29-argument** dependency list
(`src/server/runtime.rs:206-237`). jcode's own audit names this as the chief
weakness — `docs/SERVER_SERVICE_SPLIT_PLAN.md` proposes splitting into five
in-process services (session / client / swarm / debug / maintenance) "without
changing the single-process runtime model" (`:7-15`), explicitly to "stop the
spread of 20+ argument lists" (`:487`). The same doc concedes "`Server` owns
nearly all shared state in one struct" (`:28`), "`handle_client()` … receive
very wide dependency lists" (`:30`), maintenance loops "mutate the same raw
maps used by client, session, swarm, and debug paths" (`:31`), and "debug paths
bypass future boundaries" (`:173`).

### Event fanout — targeted per-session, with a global bus for UI cross-cuts

A common misreading (corrected here after source review) is that jcode
broadcasts every event to every client. It does not. jcode's **primary**
session-event delivery is **targeted**: `fanout_session_event`
(`src/server/state.rs:320-350`) delivers a `ServerEvent` only to a specific
session's `member.event_txs`, used at ~26 call sites across `client_actions.rs`,
`comm_control.rs`, `swarm.rs`, `background_tasks.rs`, `debug.rs`, etc. The
process-wide `Bus::global()` (a single `broadcast::channel(256)`,
`src/bus.rs:415-422`) carries only **cross-cutting UI** events
(`ModelsUpdated`, `BatchProgress`, `SidePanelUpdated`, `CompactionFinished`,
`MermaidRenderCompleted`); each client subscribes and filters client-side by
`session_id` (`BatchProgress`/`SidePanelUpdated` checks at
`client_lifecycle.rs:756-765`; `bus.rs is_visible_to_session`). So the
O(clients × events) cost applies only to the low-volume UI-cross-cut stream,
not to the high-volume agent-streaming path. **The one real weakness here:** all
of these senders are `mpsc::UnboundedSender` (`state.rs:83,142,144,297`) — a
slow client cannot apply backpressure and can grow server memory unbounded.

### Session takeover / reconnect ownership state machine

`Subscribe` / `ResumeSession` carry `target_session_id`, `client_instance_id`,
`client_has_local_history`, `allow_session_takeover`
(`wire.rs:79-126`). On reattach to a live session, the server runs a real
decision (`client_session.rs:1070-1162`):

```text
can_take_over_live_session = allow_session_takeover
    && (same_client_instance || (client_has_local_history && !distinct_client_instances))
```

On takeover it removes the prior owner from `client_connections`, fires its
`disconnect_tx` (`:1101-1136`), and **transfers in-flight processing**
(`is_processing` + `current_tool_name`, `:1124-1125`) to the new owner. A
*distinct* live instance is rejected with an `Error` event to avoid split-brain
(`:1138-1162`). The server can also push `SessionCloseRequested { reason }`
(`wire.rs:826`). Late-joining / reconnecting clients can replay recent activity
from a bounded `MAX_EVENT_HISTORY = 5000` swarm event-history `VecDeque`
(`state.rs:289`, `swarm.rs:687-690`). Client-side reconnect is exponential
backoff 1→30 s, resumes the same session, and may re-exec a stale client binary
(`docs/SERVER_ARCHITECTURE.md:90-98`).

### Protocol surface

One enum `Request` with **~70 variants** and `ServerEvent` with **~61
variants** (`wire.rs`), all `#[serde(tag="type")]`. Beyond the agent turn
(`Message`/`Cancel`/`SoftInterrupt`/`Compact`/`Rewind`), it carries
**provider control at the wire level** (`SetModel`/`CycleModel`/
`SetReasoningEffort`/`SwitchAnthropicAccount`/`SetServiceTier`), multi-session
ops (`Split`/`Transfer`/`ResumeSession`/`RenameSession`), and a large `Comm*`
swarm sub-protocol fused into the same enum (`CommSpawn`/`CommStop`/
`CommProposePlan`/`CommAwaitMembers`/`CommAssignTask`…).
`is_lightweight_control_request()` (`crates/jcode-protocol/src/lib.rs:446-475`)
matches `Ping` + ~24 `Comm*` control variants so they bypass the heavy
per-session attach path; the connection loop uses a `biased` `tokio::select!`
that prioritizes direct client I/O over the background bus
(`client_lifecycle.rs:617-621`, comment at `:606-609,619`) so subscribe / ping /
message can't be starved by noisy swarm file-activity. `KvCacheRequest` even
streams prompt-shape hashes to remote clients for cache-miss diagnosis
(`wire.rs:628-649`).

### Perf claim, verified context

The README's "~10 MB extra RAM per added session" is backed by a measured
PSS-per-session table (`README.md:222-246`: jcode ~9.9 MB embedding-off /
~10.4 MB, vs Claude Code ~212.7 MB, Codex CLI ~21.6 MB). This is plausible
precisely *because* of the single shared Rust process: an added session is an
`Arc<Mutex<Agent>>` + registry entries + a per-session sender, not a new OS
process.

---

## coco-rs approach

coco-rs does **not** have a single-server / multi-client peer architecture
today. The space is covered by three production pieces at different maturity
levels, plus one design doc for the true equivalent.

### (1) `hub/` — read-only event-aggregation / observability (implemented, simplified mode)

A standalone Axum web server (`hub/server/src/lib.rs`) surfaces sessions for an
operator UI. The default backend `LocalSessionJsonStore` is a **read-only
adapter over transcript JSONL** (`<memory-base>/projects/*/*.jsonl`): it
implements only the read half of the `EventStore` trait
(`list_instances`/`get_instance`/`list_sessions`/`list_events`/`search`) and
reports `health{read_only:true}` (`local_store.rs:398`). All routes are `GET`
(`hub/server/src/routes.rs:54-76`). The hub crate's `CLAUDE.md` states the
invariant: "transcript JSONL remains the source of truth … simplified mode must
not write derived hub state."

The wire protocol crate `coco-hub-protocol` **does** define a full ingest path —
`HubFrame::{Announce, AnnounceAck, Batch, BatchAck, Error}`
(`hub/protocol/src/lib.rs:12-18`), `EventEnvelope { instance_id, session_id,
seq, ts, schema_version, payload }` (`:61-70`), and `EventStore::ingest_batch`
(`hub/server/src/store/mod.rs:335-343`) — but it is wired off:
`/v1/protocol` hardcodes `read_only:true, ingest_supported:false,
live_supported:false` (`routes.rs:82-91`), `LocalSessionJsonStore::ingest_batch`
returns `NotSupported("local session json store is read-only")`
(`store/mod.rs:340-342`), and `hub/connector` is a **one-line re-export stub**
(`hub/connector/src/lib.rs:1`, deps = only `coco-error` + `coco-hub-protocol`).
**No agent-side code emits hub frames** — a grep for
`coco_hub_connector`/`ThreadEventBus`/`HubFrame`/`EventEnvelope` across
`app/`+`core/`+`services/` returns **zero non-test hits**. So the hub today =
browse-your-own-finished-transcripts, not a live multi-client server.

### (2) SDK server — single-session, stdio NDJSON (implemented)

`app/cli/src/sdk_server/` is an NDJSON-over-stdio control loop. Its state is
explicitly single-session: `SdkServerState { session: RwLock<Option<SessionHandle>>, … }`
with the comment "Only one concurrent session per server — mirrors TS where
`structuredIO.ts` holds a single `currentSession` slot"
(`sdk_server/handlers/mod.rs:236-242`). There is no `HashMap<ConnId, ConnState>`,
no per-request routing, one MCP manager
(`mcp_manager: RwLock<Option<…>>`, `:306`); `session/archive` →
`session/start` recycles the one slot. The concurrent-app-server plan further
notes `QueryEngine` is currently rebuilt **per turn**
(`concurrent-app-server-plan.md:391`), looser than codex/jcode per-session
ownership.

### (3) `coco-remote` — client to Anthropic cloud CCR (designed)

`crate-coco-remote.md` is a **client** of a remote Anthropic-hosted session over
`wss://api.anthropic.com/v1/sessions/ws/{id}/subscribe` (`:145`) with
reconnect / heartbeat (`:144-160`), plus an upstream TCP-CONNECT→WebSocket proxy
(`:83-137`). This is the **opposite direction** from jcode: coco-rs is the
client connecting *out* to a cloud worker, not a server hosting local peers.

### (4) The true multi-client concurrent server — design doc only, NOT implemented

`docs/coco-rs/concurrent-app-server-plan.md` is a 1053-line plan to add five
crates (`app/server-transport`, `app/server-protocol`, `app/server`,
`app/server-client`, `app/thread`). It specifies a two-task `MessageProcessor`
(`HashMap<ConnectionId, ConnectionState>` + outbound router), a `ThreadManager`
keyed by `ThreadId`, a `RequestSerializationQueue` with scoped access, per-thread
MCP isolation, three transports (WS / WS-over-UDS / stdio NDJSON), and a
two-level identity model. But `ls app/` shows only `cli, query, session, state,
tui` — **none of `app/server`, `app/thread`, `app/server-transport` exist**, and
the `coco daemon` subcommand is a non-functional stub: `Commands::Daemon` prints
"Daemon mode is not yet fully implemented" (`app/cli/src/main.rs:155-158`). PRs
1-13 are unstarted. This is the coco-rs equivalent of what jcode already ships.

Notably, the plan starts from the *right* boundaries:

- **Bounded channels everywhere** — `transport_event_tx: mpsc(128)` with a
  JSON-RPC `-32001` "Server overloaded" rejection on overflow (`:657`),
  per-connection writer `mpsc(256)` (WS/UDS) / `mpsc(128)` (stdio) (`:658-659`),
  per-thread `submission_tx: mpsc(64)`, per-thread `event_tx: mpsc(512)`
  (`:659-660`). This is strictly better than jcode's unbounded senders.
- **Per-Thread subscriber registry** —
  `Thread.subscribers: RwLock<HashMap<ConnectionId, mpsc::Sender<ServerNotification>>>`
  (`:564`), routed by `(thread_id → Arc<Thread>) → its subscribers`; each
  `Thread` owns its `event_tx` rather than sharing a process-wide bus (`:503-504`).
  The process-wide `ThreadEventBus` is reserved strictly for the Hub connector
  fan-in (D5, `:319,535`).
- **Disciplined two-level identity** — `ThreadId` (physical conversation) +
  `SessionId` (logical session-tree), both UUID-v7 newtypes with lossless `From`
  conversions (`:41-51`), sub-agent `SessionId` inheritance and `forked_from_id`
  lineage (`:73-104`); it explicitly **deletes** the rotating
  `Arc<RwLock<String>> session_id` footgun at `session_runtime.rs:474`
  (`:160,250`).
- **Serialization scopes** — reads are `SharedRead` (`thread/list`,
  `config/read`, `:680,689`), `turn/interrupt` is a no-scope `tokio::spawn`
  direct path (`:685`); only mutating ops take `Exclusive`.

---

## Head-to-head comparison

| Dimension | jcode (shipped) | coco-rs (today / planned) |
|---|---|---|
| Persistent multi-client server | **Yes**, one process owns N sessions (`socket.rs`, `client_lifecycle.rs`) | SDK server is single-session (`handlers/mod.rs:236`); multi-client is a doc only |
| Transports | Unix socket + Windows named pipe; **no TCP/WS server** (`transport/unix.rs`) | Plan mandates 3 from day one: stdio NDJSON, WS-over-UDS, WS-over-TCP + `InProcessClientHandle` (`plan:390,403`) |
| Daemon startup | `pipe()` ready-fd + `flock` lock + stderr race reconcile (`socket.rs:163-251,104-128,306-359`) | No daemon (`main.rs:155-158`); plan has no readiness/lock spec |
| Hot-reload | In-place `exec()` same socket/PID, `ReloadProgress` streamed (`reload.rs:160-163`, `wire.rs:950`) | None; tied to self-dev which coco-rs does not pursue |
| Idle / owner-pid lifecycle | Idle-timeout + `kill(pid,0)` liveness + on-disk metadata (`lifecycle.rs:138-204,20-132`) | No shared daemon to manage; plan silent on idle/owner-liveness |
| Event fanout | Targeted per-session `event_txs` (`state.rs:320-350`) + global UI bus (`bus.rs:415-422`); **unbounded** senders | Planned per-Thread subscriber registry (`plan:564`); **bounded** channels (`plan:657-660`) |
| Reconnect / takeover | Full state machine: supersede stale owner, transfer in-flight, reject split-brain (`client_session.rs:1070-1162`) | Plan: duplicate-resume → `Conflict` only (`plan:682,992`); no takeover/transfer specified |
| Recent-event replay on reconnect | Bounded `MAX_EVENT_HISTORY=5000` ring (`state.rs:289`) | No in-memory ring; only disk JSONL via thread archive |
| Identity model | Bare `session_id: String` threaded everywhere; no thread/session split | Two-level `ThreadId`/`SessionId` UUID-v7 (`plan:41-104`) — type-safer |
| Protocol shape | One enum, ~70 Req / ~61 Evt; provider-control + swarm fused at wire (`wire.rs`) | Plan keeps provider concerns in `vercel-ai-*`; swarm isolated; per-crate protocol |
| Layering | Flat `src/server/`; `Server` god-struct + 29-arg `handle_client` (own audit flags it) | Plan: `app/server` (L4) → `app/thread` (L3) → `app/session` (L2) from the start |

### Where jcode is genuinely ahead, and the mechanism

1. **It actually ships the persistent multi-client server.** The single
   biggest gap. But it is the explicit subject of an in-flight coco-rs plan, so
   it is a **roadmap gap, not an architecture refusal**.
2. **In-place `exec()` hot-reload preserving the socket** (`reload.rs:160-163`).
   coco-rs has nothing comparable; this is tightly coupled to jcode's self-dev
   story and to *having a long-lived server to reload into* — coco-rs pursues
   neither today.
3. **Hardened daemon-startup handshake** (`socket.rs:163-251`). The
   `pipe()`+`JCODE_READY_FD` ready signal means the parent connects exactly when
   accept loops are live; combined with `flock` + stderr race reconciliation,
   it's a "exactly one daemon, connect when ready" primitive. (Note: the *lock*
   half is not novel — codex-rs already ships `flock`+`daemon.lock`; only the
   ready-fd handshake is jcode-distinct. See M04-S1.)
4. **Session takeover / reconnect ownership** (`client_session.rs:1070-1162`).
   Concretely solves "two windows, same session, who controls it." coco-rs's
   plan only rejects duplicate resume.
5. **Idle-timeout + owner-pid liveness** (`lifecycle.rs:138-204`) — resource
   hygiene for a long-lived shared process coco-rs doesn't yet have.

### Where jcode's design would NOT fit coco-rs cleanly

- **The `Server` god-struct + 29-arg `handle_client`** (`runtime.rs:206-237`)
  is exactly the layering coco-rs's CLAUDE.md forbids. coco-rs must follow its
  layered-crate plan (`app/server` L4 depending on `app/thread` L3), not copy
  jcode's flat `src/server/`. jcode's own `SERVER_SERVICE_SPLIT_PLAN.md` is
  retrofitting boundaries coco-rs's plan starts from.
- **The fused `Comm*` swarm sub-protocol** (one socket, one enum). coco-rs
  keeps swarm in its coordinator/state layers and event aggregation isolated in
  `hub/`; bridging the full swarm taxonomy into one wire enum would violate the
  documented "isolated event streams stay isolated" rule.
- **Wire-level provider control** (`SwitchAnthropicAccount`, `SetServiceTier`,
  `KvCacheRequest`). coco-rs deliberately keeps provider concerns in
  `vercel-ai-*` crates, so these would not map 1:1 onto a coco-rs protocol.

### Perf convergence

jcode's "~10 MB per added session" (`README.md:222`, measured) is a direct
consequence of *one shared Rust process*. coco-rs's *current* model (one process
per TUI, single-session SDK server) cannot reach that for N concurrent sessions
— but coco-rs's *target* (the plan's §6 sharing table: process-wide
`AuthManager`/`ToolRegistry`/`CommandRegistry`/`McpManager`-config, per-thread
engine/MCP/exec) is architecturally the **same** single-process-shared-singletons
design. If executed, the plan converges on jcode's memory profile.

---

## Where coco-rs already matches or wins

1. **Transport breadth is designed-in, not retrofitted.** The plan mandates
   **three** transports from day one (stdio NDJSON, WS-over-UDS, WS-over-TCP)
   with a uniform `TransportEvent::IncomingMessage` so the processor is
   transport-agnostic (`plan:390,403,632`). jcode is **Unix-socket-only**
   (`transport/unix.rs`) plus a Windows pipe; it cannot be attached-to over the
   network without an external bridge. For IDE plugins / browser UIs, coco-rs's
   planned model is more general.

2. **Disciplined two-level identity.** coco-rs adopts `ThreadId` (physical) vs
   `SessionId` (logical) with lossless conversions, sub-agent inheritance, and
   `forked_from_id` lineage, and explicitly *deletes* the rotating
   `Arc<RwLock<String>> session_id` footgun (`plan:41-104,160,250`). jcode
   threads a bare `session_id: String` through the entire `Request`/`ServerEvent`/
   swarm surface with no thread/session distinction. coco-rs's model is type-safer.

3. **Bounded backpressure end-to-end (the clearest win).** jcode uses
   `mpsc::UnboundedSender` for per-client and per-session delivery
   (`state.rs:83,142,144,297`) — a slow client cannot apply backpressure and can
   grow server memory unbounded. coco-rs's plan bounds **every** hop (transport
   128, per-conn writer 256/128, per-thread `event_tx` 512) with an explicit
   `-32001` overload rejection on overflow (`plan:657-660`). This is the superior
   design.

4. **Event fanout routing precision (planned).** jcode already does targeted
   per-session fanout for the high-volume path (`state.rs:320-350`), so this is
   *not* a "jcode broadcasts everything" win. But coco-rs's plan makes the
   per-Thread subscriber registry the *only* client-delivery path and reserves
   the process-wide bus strictly for Hub fan-in (`plan:535,564`) — a cleaner
   separation than jcode's mixed "targeted fanout + global UI bus" model.

5. **Read-only observability is correctly factored.** coco-rs's `hub/` keeps the
   wire protocol (`coco-hub-protocol`) free of Axum/SQLite/UI deps, makes routes
   depend on an `EventStore` trait, and enforces "transcript JSONL is the source
   of truth." jcode has no separate read-model layer — inspection goes through
   the same privileged debug socket and `debug_*` modules that mutate live state
   (`SERVER_SERVICE_SPLIT_PLAN.md:173`). For an operator dashboard, coco-rs's
   separation is cleaner.

6. **jcode's own docs concede the central weakness.** `Server` "owns nearly all
   shared state in one struct" (`SERVER_SERVICE_SPLIT_PLAN.md:28`),
   `handle_client()` "is both connection loop and application router" (`:131`),
   maintenance loops "reach into domain maps directly" (`:159`). coco-rs's planned
   `app/server` (L4) → `app/thread` (L3) → `app/session` (L2) layering starts
   from the boundaries jcode is retrofitting.

---

## Optimization recommendations for coco-rs (adversarially verified)

All six analyst suggestions survived adversarial review; **none was refuted**
(two confirmed outright, four are nuanced with the correction folded in). Three
strong verifier findings are appended as supplementary recommendations. Every
item respects coco-rs's documented non-goals. **All are gated on the
concurrent-app-server plan actually shipping** (PRs 9/11) — there is no shared
daemon to harden today.

### M04-S3 — Reconnect-ownership / session-takeover state machine *(confirmed)*

- **Why.** jcode cleanly resolves "two clients want the same live session":
  supersede the stale owner, transfer in-flight processing, reject split-brain,
  using `client_instance_id` + `client_has_local_history` +
  `allow_session_takeover` (`wire.rs:79-126`; decision at
  `client_session.rs:1070-1162`, transfer at `:1124-1125`, reject at
  `:1138-1162`). coco-rs's plan provides only split-brain **rejection** —
  `thread/resume` is `Global Exclusive` and the only concurrent-resume test
  asserts the second caller gets a `Conflict` (`plan:682,992`); the SDK server
  today is single-session (`handlers/mod.rs:236`). There are no
  takeover/transfer semantics anywhere.
- **Concrete change.** In `app/server` (PR 9): on `session/resume` or a second
  subscribe to an already-attached `ThreadId`, drive a small `AttachDecision`
  enum (`RejectConflict` / `TakeOver` / `PassiveObserve`). For `TakeOver`, signal
  the prior `ConnectionId`'s subscriber to detach and move `active_turn`
  ownership; **default to `RejectConflict`** (matches the plan's split-brain
  stance). Carry `client_instance_id` + `allow_takeover` in `session/resume`
  params (mirroring jcode's wire fields). Keep `thread_id` server-internal (D10)
  — key the decision on `ConnectionId` + `SessionId`.
- **Impact / effort / risk.** Impact medium; effort **high**; risk
  concurrency-heavy — must avoid deadlock between the per-Thread
  `Mutex<ThreadState>` and the subscriber registry. jcode does it under a single
  struct; coco-rs must do it across the `app/server`/`app/thread` boundary.

### M04-S6 — Wire the agent-side Hub connector (CoreEvent → `coco-hub-protocol` Batch) *(confirmed)*

- **Why.** jcode's server is itself the live multi-session observation point —
  global `Bus` (`bus.rs:415-422`) for cross-cuts plus targeted
  `fanout_session_event` (`state.rs:320-350`) deliver live streaming events to
  attached clients; there is no "replay only" limitation. coco-rs has the full
  ingest wire protocol designed (`HubFrame::{Announce,Batch,…}`,
  `protocol/src/lib.rs:12-18`; `EventStore::ingest_batch`, `store/mod.rs:335-343`)
  but **no producer**: `/v1/protocol` is `ingest_supported:false,
  live_supported:false` (`routes.rs:82-91`), `LocalSessionJsonStore::ingest_batch`
  returns `NotSupported` (`store/mod.rs:340-342`), `hub/connector` is a 1-line stub
  (`connector/src/lib.rs:1`), and grep for the connector types across
  `app/`+`core/`+`services/` is empty. So coco-rs's hub can only replay finished
  JSONL today.
- **Concrete change.** Implement `hub/connector` per its `CLAUDE.md` + plan
  D5/D11: a process-wide `ThreadEventBus` (mpsc fan-in) that subscribes to each
  Thread's `CoreEvent` stream, frames events as `EventEnvelope` with
  `{thread_id, session_id, seq}` (`plan:319,325,535`), ring-buffers (bounded),
  and streams `HubFrame::Batch` over WebSocket to the hub server. Add a writable
  `EventStore` impl (or flip `ingest_supported` in non-simplified mode) so the
  hub can hold live rows. This turns the hub from transcript-replay into a live
  operator view.
- **Impact / effort / risk.** Impact **high**; effort high; risk: must respect
  the documented non-goal that `RetrievalEvent`/provider callbacks stay
  un-bridged — bridge only the `CoreEvent` taxonomy via the single aggregate sink
  (D5). Bounded buffering + backpressure required so a slow hub can't stall agent
  turns; **keep simplified read-only mode the default** so the JSONL-is-truth
  invariant holds when the connector is off.

### M04-S1 — Ready-fd handshake for the planned detached `coco serve` daemon *(nuanced)*

- **Why.** jcode connects exactly when the daemon can serve via the
  `pipe()`+`JCODE_READY_FD` signal (`socket.rs:163-251`), guarantees a single
  daemon via `flock` (`socket.rs:104-128`), and reconciles spawn races via stderr
  (`socket.rs:306-359`). coco-rs has no daemon today (`main.rs:155-158`) and the
  plan specifies `coco serve --listen …` (`plan:390`) with **no** readiness
  handshake and **no** single-daemon lock (grep of the plan returns nothing).
- **Correction folded in (the analyst overstated "net-new beyond codex").**
  codex-rs/app-server-daemon **already** ships `flock`+`daemon.lock`
  (`codex-rs/app-server-daemon/src/lib.rs:32`) and `setsid()`, plus
  `wait_until_ready` / `connect_retry` polling (`lib.rs:442,211`). So the
  **single-daemon-lock half is a straight codex port — do not reinvent it.** Only
  jcode's `pipe`/ready-fd handshake is genuinely distinct (codex merely polls).
  Also, the plan's *primary* topology is **TUI-embeds-server-in-process via
  `InProcessClientHandle`** (`plan:336,390,403`) — an in-memory mpsc pair, no
  spawn — so this handshake matters **only** for the detached `coco serve` path
  that a separate `coco` process must spawn-and-connect.
- **Concrete change.** In PR 9/11, port codex's `flock`+`daemon.lock` guarantee,
  and **add** jcode's `pipe()`+`COCO_SERVER_READY_FD` signal (emitted after the
  inbound/outbound tasks bind) as the new piece. The `InProcessClientHandle` path
  skips this entirely. Mirror jcode's stderr-race reconciliation. `cfg(unix)`-gate
  and replicate jcode's CLOEXEC discipline (`socket.rs:178-196`) so the parent
  doesn't hang to timeout.
- **Impact / effort / risk.** Impact medium; effort medium; risk: Unix-only; the
  write-end CLOEXEC-clear across `exec` is the load-bearing detail. *(Aside: the
  plan already cites codex verbatim (`plan:400-401`), in tension with the
  project's "never propose codex-rs as a design reference" rule — reuse-vs-mirror
  is a team judgment call here.)*

### M04-S4 — Idle-timeout + owner-pid liveness for the shared daemon *(nuanced)*

- **Why.** jcode's long-lived server reclaims itself: exits after configurable
  idle with zero clients and self-terminates if its owner client dies
  (`lifecycle.rs:138-204`; `process_alive` = `kill(pid,0)`, `:217-232`). coco-rs
  has no shared daemon to manage today, and the plan covers only in-process
  Thread lifecycle — no idle-shutdown / owner-liveness (grep empty). The hub is a
  read-only web server with no such policy.
- **Correction folded in.** jcode hard-codes `JCODE_SERVER_OWNER_PID` /
  `JCODE_TEMP_SERVER_IDLE_SECS` env (`lifecycle.rs:9-10`), which **violates
  coco-rs's config rule** ("never call `std::env::var` ad-hoc; add to `EnvKey`").
  Surface the knob via `RuntimeConfig` (a new `server.idle_timeout_secs`
  sub-config) and a `COCO_`-prefixed `EnvKey` (`COCO_SERVER_OWNER_PID`), never raw
  env. Optionally port codex-rs/app-server-daemon's existing restart/readiness
  scaffolding (`RestartMode`, `wait_until_ready`) instead of greenfield.
- **Concrete change.** Add an idle monitor task to the future `coco serve`
  daemon: track live `ConnectionId` count in `MessageProcessor`; after a
  configurable idle window with zero connections, drain `ThreadManager` (archive
  threads, flush transcripts) and exit. **Gate shutdown strictly on
  `active_turn.is_none()` across all threads — never kill mid-turn.** Owner-pid
  check `cfg(unix)`-gated.
- **Impact / effort / risk.** Impact **low** (only matters once a detached daemon
  ships); effort low; risk: the mid-turn-kill guard is essential.

### M04-S5 — `biased`-select so control RPCs aren't starved by streaming *(nuanced)*

- **Why.** jcode classifies cheap control requests via
  `is_lightweight_control_request()` (Ping + ~24 `Comm*`, `lib.rs:446-475`) and
  uses a `biased` `tokio::select!` prioritizing direct client I/O over the
  background bus (`client_lifecycle.rs:617-621`).
- **Correction folded in (the gap is narrower than stated).** coco-rs's plan
  **already** classifies cheap reads correctly: `thread/list` and `config/read`
  are `SharedRead` (many readers concurrent, `plan:680,689`), `turn/interrupt` is
  a no-scope `tokio::spawn` direct path (`plan:685`). So "reads enqueue on a serial
  Exclusive queue and get starved" is **mostly refuted** — **do not re-add the
  fast-path classification, it exists.** The genuinely-missing piece is only
  jcode's `biased`-select pattern: today the plan's inbound processor reads a
  single `transport_event_tx(128)` with no priority between client RPCs and any
  bus-sourced work (`plan:632`).
- **Concrete change.** **If/when** the inbound processor select drains a Hub/bus
  channel alongside client RPCs (i.e. once M04-S6 wires the connector fan-in),
  bias it toward client RPCs (jcode's `biased` pattern) so a streaming-heavy
  thread can't starve a control request on another connection. This is a
  guardrail for that future wiring, **not** an independent change. Ensure any
  fast-path read takes only `SharedRead` snapshots (`plan:693` reentrancy note).
- **Impact / effort / risk.** Impact low; effort low; risk minimal.

### M04-S2 — Per-Thread routing as the *only* client-delivery path *(nuanced — guardrail only)*

- **Why.** The premise that "jcode fans every event to every client over one
  global broadcast" is **refuted by source**: jcode's primary high-volume path is
  the targeted `fanout_session_event` (`state.rs:320-350`); the global
  `broadcast(256)` (`bus.rs:415-422`) carries only low-volume UI cross-cuts,
  filtered client-side (`client_lifecycle.rs:756-765`). And coco-rs's plan
  **already** specifies the correct per-Thread registry (`Thread.subscribers`,
  `plan:564`) with `ThreadEventBus` reserved for Hub fan-in only (`plan:535`). So
  this is **advisory only**, not a real gap.
- **Concrete change (guardrail).** In PR 6 (`Thread` split), make
  `Thread.subscribers` the **sole** client-delivery path; route by
  `(thread_id → Arc<Thread>) → its subscribers`. Reserve any process-wide bus
  strictly for the Hub connector fan-in, never for client delivery. Add the
  §8.1 `thread_manager.test.rs` isolation assertion that events for thread A never
  reach a subscriber of thread B.
- **Fairness note.** jcode uses **unbounded** mpsc for client delivery
  (`state.rs:83,142,144`) — a backpressure weakness; coco-rs's bounded
  `event_tx: mpsc(512)` (`plan:660`) is the better design.
- **Impact / effort / risk.** Impact medium; effort medium; risk low — the only
  risk is an implementer taking a broadcast shortcut.

### Supplementary recommendations from verifier findings

- **VF-1 — In-memory recent-event ring for reconnect/observe.** jcode keeps a
  bounded `MAX_EVENT_HISTORY = 5000` swarm event-history `VecDeque`
  (`state.rs:289`, `swarm.rs:687-690`) so late-joining / reconnecting clients can
  replay recent activity. coco-rs's plan has **no** in-memory recent-event ring —
  reconnect recovery relies solely on disk JSONL via thread archive. *Change:* pair
  with M04-S3 — give each `Thread` a small bounded ring (e.g. last-N
  `ServerNotification`s) so a reconnecting client (or a `PassiveObserve` attach)
  gets immediate context without a full transcript re-read. *Impact low; effort
  low; risk: keep it bounded so a long-lived thread can't grow it unboundedly.*

- **VF-2 — On-disk daemon-discovery metadata.** jcode writes a
  `<socket>.server.json` carrying `schema_version/scope/pid/ppid/owner_pid/
  idle_timeout/argv` (`lifecycle.rs:20-132`) for introspection and orphan
  reconciliation. coco-rs has no daemon-discovery metadata design. *Change:* when
  the detached `coco serve` daemon ships (PR 9/11), write an analogous
  `<socket>.coco-server.json` under the runtime dir (paths via `RuntimeConfig`,
  not ad-hoc) so `coco ps`/`coco attach` and race-reconciliation have an
  authoritative discovery record. Pairs naturally with M04-S1/S4. *Impact low;
  effort low; risk minimal.*

- **VF-3 (fairness, coco-rs already better) — bounded backpressure.** Already
  surfaced under M04-S2 and "Where coco-rs wins #3": jcode's unbounded per-client/
  per-session senders (`state.rs:83,142,144,297`) are a memory-growth risk under a
  slow client; coco-rs's plan bounds every hop with `-32001` overload rejection
  (`plan:657-660`). No action needed — keep the plan's bounded design; do not
  regress to unbounded for "simplicity."

---

## Rejected after adversarial review

**None.** All six analyst suggestions survived: M04-S3 and M04-S6 **confirmed**
outright; M04-S1, M04-S2, M04-S4, M04-S5 **nuanced** (kept with corrections
folded into the recommendations above). The corrections that materially narrowed
scope — and which readers should treat as "checked and bounded" rather than
dropped — are:

- **M04-S1:** the single-daemon `flock` lock is *not* net-new vs codex (codex
  already ships `daemon.lock`, `app-server-daemon/src/lib.rs:32`); only jcode's
  ready-fd handshake is distinct. And it applies only to the *detached* `coco
  serve` path — the primary `InProcessClientHandle` topology needs no handshake.
- **M04-S2:** the premise "jcode broadcasts everything" is **false** — jcode's
  main path is targeted per-session fanout (`state.rs:320-350`); coco-rs's plan
  already specifies the correct per-Thread registry. Demoted to a guardrail.
- **M04-S4:** must route the idle/owner knobs through `RuntimeConfig` + a
  `COCO_`-prefixed `EnvKey`, not jcode's ad-hoc env (`lifecycle.rs:9-10`), per
  coco-rs config rules.
- **M04-S5:** the read/ping/interrupt fast-path **already exists** in the plan
  (`SharedRead`, `plan:680,685,689`); only jcode's `biased`-select cross-source
  starvation guard is missing, and only once a Hub/bus channel is added to the
  processor select.
